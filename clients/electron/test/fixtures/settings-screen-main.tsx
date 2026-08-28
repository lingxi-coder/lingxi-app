import { StrictMode, useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import type { SettingsSnapshotEvent } from '../../src/renderer/bridge/useBridge';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

function Fixture() {
  const [hasProject, setHasProject] = useState(false);
  const [running, setRunning] = useState(false);
  const [settingsSnapshotEvent, setSettingsSnapshotEvent] = useState<SettingsSnapshotEvent | null>(null);
  const [restartCalls, setRestartCalls] = useState(0);
  const [refreshCalls, setRefreshCalls] = useState(0);
  const [closeCalls, setCloseCalls] = useState(0);

  // `useCallback` with empty deps keeps these referentially stable across
  // Fixture re-renders — `SettingsScreen` depends on `bridge.refreshSettingsSnapshot`'s
  // identity in an effect, and a fresh function every render would refire it forever.
  const restartBridge = useCallback(async () => { setRestartCalls((n) => n + 1); }, []);
  const refreshSettingsSnapshot = useCallback(async () => { setRefreshCalls((n) => n + 1); }, []);

  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      workspace: hasProject ? { path: '/test/project', trusted: true } : { trusted: false },
    },
    connected: true,
    running,
    settingsSnapshotEvent,
    restartBridge,
    refreshSettingsSnapshot,
  };

  useEffect(() => {
    window.__settingsScreenTest = {
      selectPage: (id: string) => {
        (document.querySelector(`[data-nav-page="${id}"]`) as HTMLButtonElement | null)?.click();
      },
      setHasProject,
      setRunning,
      setSnapshot: (effective: Record<string, unknown>, active: Record<string, unknown>) => {
        setSettingsSnapshotEvent({
          type: 'settings_snapshot',
          effective_json: JSON.stringify(effective),
          provenance_json: JSON.stringify(Object.fromEntries(Object.keys(effective).map((key) => [key, 'user']))),
          active_json: JSON.stringify(active),
        } as SettingsSnapshotEvent);
      },
      setMalformedSnapshot: () => {
        setSettingsSnapshotEvent({
          type: 'settings_snapshot',
          effective_json: '{not json',
          provenance_json: '{}',
        } as SettingsSnapshotEvent);
      },
      clickRestart: () => {
        (document.querySelector('[data-testid="settings-pending-banner"] button') as HTMLButtonElement | null)?.click();
      },
      close: () => setCloseCalls((n) => n + 1),
      state: () => ({
        hasLayerSwitcher: Boolean(document.querySelector('[data-testid="layer-switcher"]')),
        userDisabled: (document.querySelector('[data-layer="user"]') as HTMLButtonElement | null)?.disabled ?? null,
        projectDisabled: (document.querySelector('[data-layer="project"]') as HTMLButtonElement | null)?.disabled ?? null,
        localDisabled: (document.querySelector('[data-layer="local"]') as HTMLButtonElement | null)?.disabled ?? null,
        hasBanner: Boolean(document.querySelector('[data-testid="settings-pending-banner"]')),
        bannerText: document.querySelector('[data-testid="settings-pending-banner"] span')?.textContent ?? null,
        restartButtonDisabled: (document.querySelector('[data-testid="settings-pending-banner"] button') as HTMLButtonElement | null)?.disabled ?? null,
        placeholderKind: document.querySelector('[data-testid="page-placeholder"]')?.getAttribute('data-placeholder-kind') ?? null,
        hasSnapshotError: Boolean(document.querySelector('[data-testid="settings-snapshot-error"]')),
        restartCalls,
        refreshCalls,
        closeCalls,
      }),
    };
    return () => { delete window.__settingsScreenTest; };
  }, [restartCalls, refreshCalls, closeCalls]);

  return (
    <Theme.Provider value={tokens('light')}>
      <SettingsScreen
        bridge={bridge as never}
        theme="light"
        onTheme={() => {}}
        onClose={() => setCloseCalls((n) => n + 1)}
      />
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __settingsScreenTest?: {
      selectPage(id: string): void;
      setHasProject(value: boolean): void;
      setRunning(value: boolean): void;
      setSnapshot(effective: Record<string, unknown>, active: Record<string, unknown>): void;
      setMalformedSnapshot(): void;
      clickRestart(): void;
      close(): void;
      state(): {
        hasLayerSwitcher: boolean;
        userDisabled: boolean | null;
        projectDisabled: boolean | null;
        localDisabled: boolean | null;
        hasBanner: boolean;
        bannerText: string | null;
        restartButtonDisabled: boolean | null;
        placeholderKind: string | null;
        hasSnapshotError: boolean;
        restartCalls: number;
        refreshCalls: number;
        closeCalls: number;
      };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
