import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

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
  settingsSnapshotEvent: null,
  refreshSettingsSnapshot: async () => undefined,
  refreshDiagnostics: async () => [],
  setProviderCredential: async () => {
    persistencePending = true;
    return new Promise<unknown>((resolve) => { resolvePersistence = resolve; });
  },
  restartBridge: async () => { restartCalls += 1; },
  setModel: async () => undefined,
  clearProviderCredential: async () => ({ providerId: 'anthropic', configured: false, encryptionAvailable: true }),
  testProviderConnection: async () => { throw new Error('test credential rejected'); },
  refreshProviderCredential: async () => undefined,
  setApiBaseUrl: async () => undefined,
  copyDiagnostics: async () => undefined,
  exportDiagnostics: async () => null,
};

function Fixture() {
  const [open, setOpen] = useState(true);

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
      },
      startConnectionTest: () => {
        const test = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === '测试连接');
        test?.click();
      },
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
    <Theme.Provider value={tokens('light')}>
      {open && (
        <SettingsScreen
          bridge={bridge as never}
          theme="light"
          onTheme={() => {}}
          initialProviderId="anthropic"
          onClose={() => { closeCalls += 1; setOpen(false); }}
        />
      )}
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
      clearStoredCredential(): void;
      connectionTestErrorVisible(): boolean;
      state(): { persistencePending: boolean; restartCalls: number; closeCalls: number; activeSessionId: string; activeWork: boolean };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
