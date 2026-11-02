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
  providerAccounts: providerAccountsMock({
    initial: [{ accountId: '11111111-1111-4111-8111-111111111111', providerId: 'anthropic', executionBindingId: 'anthropic', displayName: 'Test account', authKind: 'api_key', enabled: true, userPriority: 0, configRevision: 0, identityGeneration: 1, credentialRef: { storeScope: 'renderer-fixture', secretRef: 'acct.11111111-1111-4111-8111-111111111111' } }],
    write: async () => { persistencePending = true; await new Promise(resolve => { resolvePersistence = resolve; }); },
    probe: async () => { connectionTestCalls++; throw new Error('test credential rejected'); },
  }),
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
      startSave: () => {
        const input = document.querySelector<HTMLInputElement>('input[type="password"]');
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
        setter?.call(input, 'sk-pending-save');
        input?.dispatchEvent(new Event('input', { bubbles: true }));
        setTimeout(() => [...document.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Add API key')?.click(), 0);
      },
      resolvePersistence: () => {
        resolvePersistence?.(undefined);
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
          providerCredentialField: Boolean(document.querySelector('input[aria-label="API key"]')),
        };
      },
      connectionTestCalls: () => connectionTestCalls,
      clearStoredCredential: () => {
        const remove = [...document.querySelectorAll('button')].find((button) => button.textContent?.trim() === 'Remove');
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
