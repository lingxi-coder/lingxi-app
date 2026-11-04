import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { providerAccountsMock } from './provider-accounts-mock';
import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';

setLanguagePreference('en');

let persistencePending = false;
let resolvePersistence: ((value: unknown) => void) | undefined;
let rejectPersistence: ((reason: Error) => void) | undefined;
let removalPending = false;
let releaseRemoval: (() => void) | undefined;
const persistedCredentials: Array<{ providerId: string; credential: string }> = [];
let restartCalls = 0;
let closeCalls = 0;
let activeSessionId = 'session-a';
let activeWork = false;
let connectionTestCalls = 0;

const accounts = providerAccountsMock({
  initial: [{ accountId: '11111111-1111-4111-8111-111111111111', providerId: 'anthropic', executionBindingId: 'anthropic', displayName: 'Test account', authKind: 'api_key', enabled: true, userPriority: 0, configRevision: 0, identityGeneration: 1, credentialRef: { storeScope: 'renderer-fixture', secretRef: 'acct.11111111-1111-4111-8111-111111111111' } }],
  write: async (providerId, credential) => {
    persistencePending = true;
    try {
      await new Promise((resolve, reject) => { resolvePersistence = resolve; rejectPersistence = reject; });
      persistedCredentials.push({ providerId, credential });
    } finally { persistencePending = false; }
  },
  probe: async () => { connectionTestCalls++; throw new Error('test credential rejected'); },
});
const bridge = {
  activeSession: { projectPath: '/test/project', sessionId: activeSessionId },
  bootstrap: {
    revision: 1,
    settings: { version: 1, projects: [], pinnedSessions: [] },
    workspace: { trusted: false },
    activeSession: { projectPath: '/test/project', sessionId: activeSessionId },
    runtimes: [],
    projectCatalogs: {},
    providerAccountSnapshot: fixtureAccountSnapshot(['anthropic']),
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
  providerAccounts: {
    ...accounts,
    command: async (...args: Parameters<typeof accounts.command>) => {
      if (args[0].action === 'remove') {
        removalPending = true;
        try { await new Promise<void>(resolve => { releaseRemoval = resolve; }); }
        finally { removalPending = false; }
      }
      return accounts.command(...args);
    },
  },
  restartBridge: async () => { restartCalls += 1; },
  refreshModelPicker: async () => undefined,
  setModel: async () => undefined,
  setApiBaseUrl: async () => undefined,
  copyDiagnostics: async () => undefined,
  exportDiagnostics: async () => null,
};

function Fixture() {
  const [open, setOpen] = useState(true);
  const [fixtureSessionId, setFixtureSessionId] = useState(activeSessionId);

  useEffect(() => {
    window.__settingsTransactionTest = {
      setCredential: (value: string) => {
        const input = document.querySelector<HTMLInputElement>('dialog[open] input[type="password"]');
        if (!input) throw new Error('Account credential form is not open');
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        setter?.call(input, value);
        input.dispatchEvent(new Event('input', { bubbles: true }));
      },
      settlePersistence: (fail: boolean) => {
        if (!resolvePersistence || !rejectPersistence) throw new Error('No credential write is pending');
        if (fail) rejectPersistence(new Error('test credential storage failed'));
        else resolvePersistence(undefined);
        resolvePersistence = undefined;
        rejectPersistence = undefined;
      },
      releaseRemoval: () => {
        if (!releaseRemoval) throw new Error('No account removal is pending');
        releaseRemoval();
        releaseRemoval = undefined;
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
        const test = document.querySelector<HTMLButtonElement>('[data-testid^="provider-account-test-"]');
        if (!test) throw new Error('Anthropic provider connection test button is not mounted');
        if (test.disabled) throw new Error('Anthropic provider connection test button is disabled');
        test.click();
      },
      connectionTestButtonState: () => {
        const button = document.querySelector<HTMLButtonElement>('[data-testid^="provider-account-test-"]');
        return {
          exists: Boolean(button),
          disabled: button?.disabled ?? true,
          accountCount: document.querySelectorAll('.pa-account').length,
        };
      },
      connectionTestCalls: () => connectionTestCalls,
      connectionTestErrorVisible: () => document.body.innerText.includes('test credential rejected'),
      state: () => ({ persistencePending, removalPending, restartCalls, closeCalls, activeSessionId, activeWork, persistedCredentials: [...persistedCredentials] }),
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
      setCredential(value: string): void;
      settlePersistence(fail: boolean): void;
      releaseRemoval(): void;
      switchSessionAndStartWork(): void;
      startConnectionTest(): void;
      connectionTestButtonState(): { exists: boolean; disabled: boolean; accountCount: number };
      connectionTestCalls(): number;
      connectionTestErrorVisible(): boolean;
      state(): { persistencePending: boolean; removalPending: boolean; restartCalls: number; closeCalls: number; activeSessionId: string; activeWork: boolean; persistedCredentials: Array<{ providerId: string; credential: string }> };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
