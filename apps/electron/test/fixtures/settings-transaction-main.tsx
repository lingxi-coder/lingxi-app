import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';

// The interaction helpers select the Chinese labels below.
localStorage.setItem(LANGUAGE_STORAGE_KEY, 'zh-Hans');

// Task 20: this fixture used to mount `BetaSettings` directly. That
// component (and the modal it lived in) is gone — `ProviderCredentials.tsx`
// (`src/renderer/components/settings/pages/ProviderCredentials.tsx`) is the
// same already-tested transaction logic (`isCurrentCredentialTransaction`,
// `persistProviderCredentialAndApplyModel`), lifted verbatim, now reached
// through `SettingsScreen`. `initialProviderId="anthropic"` lands the shell
// straight on that page (`resolveInitialPage`), so this fixture drives the
// close-during-persistence behavior through the real settings shell.

let persistencePending = false;
let resolvePersistence: ((value: unknown) => void) | undefined;
let restartCalls = 0;
let closeCalls = 0;
let activeSessionId = 'session-a';
let activeWork = false;
let connectionTestCalls = 0;

const bridge = {
  activeSession: { projectPath: '/test/project', sessionId: activeSessionId },
  bootstrap: {
    revision: 1,
    settings: { version: 1, projects: [], pinnedSessions: [] },
    workspace: { trusted: false },
    activeSession: { projectPath: '/test/project', sessionId: activeSessionId },
    runtimes: [],
    projectCatalogs: {},
    providerCredentials: [{ providerId: 'anthropic', configured: true, encryptionAvailable: true, credentialPreview: '••••test' }],
    connection: { status: 'connected' as const },
    diagnostics: [],
  },
  desktop: { currentModel: null },
  connected: true,
  connection: { status: 'connected' as const },
  running: false,
  sessionLoading: false,
  settingsSnapshotEvent: {
    type: 'settings_snapshot',
    effective_json: '{}',
    active_json: '{}',
    provenance_json: '{}',
    files_json: '[]',
    layers_json: '{"user":{},"project":{},"local":{}}',
    locked: [],
    merged_keys: [],
  },
  refreshSettingsSnapshot: async () => undefined,
  refreshDiagnostics: async () => [],
  setProviderCredential: async () => {
    persistencePending = true;
    return new Promise<unknown>((resolve) => { resolvePersistence = resolve; });
  },
  restartBridge: async () => { restartCalls += 1; },
  setModel: async () => undefined,
  clearProviderCredential: async () => ({ providerId: 'anthropic', configured: false, encryptionAvailable: true }),
  testProviderConnection: async () => {
    connectionTestCalls += 1;
    throw new Error('test credential rejected');
  },
  refreshProviderCredential: async () => undefined,
  setApiBaseUrl: async () => undefined,
  copyDiagnostics: async () => undefined,
  exportDiagnostics: async () => null,
};

function Fixture() {
  const [open, setOpen] = useState(true);
  const [fixtureSessionId, setFixtureSessionId] = useState(activeSessionId);

  useEffect(() => {
    window.__settingsTransactionTest = {
      startSave: () => {
        const input = document.querySelector<HTMLInputElement>('input[type="password"]');
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        setter?.call(input, 'sk-pending-save');
        input?.dispatchEvent(new Event('input', { bubbles: true }));
        const save = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === '保存');
        save?.click();
      },
      resolvePersistence: () => {
        resolvePersistence?.({ credential: { providerId: 'anthropic', configured: true, encryptionAvailable: true }, settings: bridge.bootstrap.settings });
        resolvePersistence = undefined;
        persistencePending = false;
      },
      switchSessionAndStartWork: () => {
        activeSessionId = 'session-b';
        activeWork = true;
        const activeSession = { projectPath: '/test/project', sessionId: activeSessionId };
        bridge.activeSession = activeSession;
        bridge.bootstrap.activeSession = activeSession;
        setFixtureSessionId(activeSessionId);
      },
      startConnectionTest: () => {
        const test = document.querySelector<HTMLButtonElement>('[data-testid="provider-connection-test"]');
        if (!test) throw new Error('Anthropic provider connection test button is not mounted');
        if (test.disabled) throw new Error('Anthropic provider connection test button is disabled');
        test.click();
      },
      connectionTestButtonState: () => {
        const button = document.querySelector<HTMLButtonElement>('[data-testid="provider-connection-test"]');
        return {
          exists: Boolean(button),
          disabled: button?.disabled ?? true,
          providerCredentialField: Boolean(document.querySelector('input[aria-label="Anthropic API key"]')),
        };
      },
      connectionTestCalls: () => connectionTestCalls,
      clearStoredCredential: () => {
        const remove = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === '删除 API Key');
        remove?.click();
      },
      connectionTestErrorVisible: () => document.body.innerText.includes('test credential rejected'),
      state: () => ({ persistencePending, restartCalls, closeCalls, activeSessionId, activeWork }),
    };
    return () => { delete window.__settingsTransactionTest; };
  }, []);

  return (
    <Theme.Provider value={tokens(false)}>
      <div data-testid="settings-transaction-owner" data-session-id={fixtureSessionId}>
        {open && (
          <SettingsScreen
            bridge={bridge as never}
            theme="light"
            onTheme={() => {}}
            initialPageId="provider-credentials"
            initialProviderId="anthropic"
            onClose={() => { closeCalls += 1; setOpen(false); }}
          />
        )}
      </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __settingsTransactionTest?: {
      startSave(): void;
      resolvePersistence(): void;
      switchSessionAndStartWork(): void;
      startConnectionTest(): void;
      connectionTestButtonState(): { exists: boolean; disabled: boolean; providerCredentialField: boolean };
      connectionTestCalls(): number;
      clearStoredCredential(): void;
      connectionTestErrorVisible(): boolean;
      state(): { persistencePending: boolean; restartCalls: number; closeCalls: number; activeSessionId: string; activeWork: boolean };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
