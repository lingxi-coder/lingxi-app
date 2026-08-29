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
// straight on that page (`resolveInitialPage`), so this fixture still drives
// the exact same close-during-persistence and stale-session-recovery
// scenarios end to end, just through the new shell instead of the old modal.

let persistencePending = false;
let resolvePersistence: ((value: unknown) => void) | undefined;
let restartCalls = 0;
let restartTargets: string[] = [];
let restartAttempts: string[] = [];
let restartErrors = 0;
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
    providerCredentials: [{ providerId: 'anthropic', configured: false, encryptionAvailable: true }],
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
  restartBridge: async (sessionId?: string) => {
    const requestedSessionId = sessionId ?? activeSessionId;
    restartAttempts.push(requestedSessionId);
    // Model the host's stale-session/active-work guard for the transaction
    // fixture. A late completion must not restart another active session.
    if (requestedSessionId !== activeSessionId || activeWork) {
      restartErrors += 1;
      throw new Error('the requested session is no longer active or has active work');
    }
    restartCalls += 1;
    restartTargets.push(requestedSessionId);
  },
  setModel: async () => undefined,
  clearProviderCredential: async () => ({ providerId: 'anthropic', configured: false, encryptionAvailable: true }),
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
        const connect = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === '连接');
        connect?.click();
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
      setSessionState: (sessionId: string, work: boolean) => {
        activeSessionId = sessionId;
        activeWork = work;
      },
      clickRetry: () => {
        const retry = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === '重试引擎连接');
        retry?.click();
      },
      state: () => ({ persistencePending, restartCalls, restartTargets: [...restartTargets], restartAttempts: [...restartAttempts], restartErrors, closeCalls, activeSessionId, activeWork }),
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
      setSessionState(sessionId: string, work: boolean): void;
      clickRetry(): void;
      state(): { persistencePending: boolean; restartCalls: number; restartTargets: string[]; restartAttempts: string[]; restartErrors: number; closeCalls: number; activeSessionId: string; activeWork: boolean };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
