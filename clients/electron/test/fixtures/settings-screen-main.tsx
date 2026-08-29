import { StrictMode, useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import type { SettingsSnapshotEvent } from '../../src/renderer/bridge/useBridge';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

// Module-scope (not `useCallback`) so these are referentially stable across
// Fixture re-renders with zero effort — none of them close over any
// component state, they only exist so the Task 16 pages have something
// callable instead of `undefined`.
async function noopAsyncVoid(): Promise<void> {}
async function noopAsyncNull(): Promise<null> { return null; }
async function noopAsyncArray(): Promise<never[]> { return []; }

function Fixture() {
  const [open, setOpen] = useState(true);
  const [hasProject, setHasProject] = useState(false);
  const [running, setRunning] = useState(false);
  const [connected, setConnected] = useState(true);
  const [sessionLoading, setSessionLoading] = useState(false);
  const [settingsSnapshotEvent, setSettingsSnapshotEvent] = useState<SettingsSnapshotEvent | null>(null);
  const [restartCalls, setRestartCalls] = useState(0);
  const [refreshCalls, setRefreshCalls] = useState(0);
  const [closeCalls, setCloseCalls] = useState(0);
  const [restartShouldFail, setRestartShouldFail] = useState<string | null>(null);

  // `restartBridge` below must stay referentially stable (see the next
  // comment), so it cannot close over `restartShouldFail` state directly —
  // an actual `useRef` (mutated in place, not a fresh object every render)
  // gives it the LATEST value without becoming a new function every time the
  // test driver flips it.
  const restartShouldFailRef = useRef<string | null>(null);
  restartShouldFailRef.current = restartShouldFail;

  // `useCallback` with empty deps keeps these referentially stable across
  // Fixture re-renders — `SettingsScreen` depends on `bridge.refreshSettingsSnapshot`'s
  // identity in an effect, and a fresh function every render would refire it forever.
  const restartBridge = useCallback(async () => {
    setRestartCalls((n) => n + 1);
    if (restartShouldFailRef.current) throw new Error(restartShouldFailRef.current);
  }, []);
  const refreshSettingsSnapshot = useCallback(async () => { setRefreshCalls((n) => n + 1); }, []);

  const bridge = {
    activeSession: { projectPath: '/test/project', sessionId: 'session-a' },
    bootstrap: {
      // `settings`/`diagnostics`/`runtimes`/`versions` below only matter to
      // the Task 16 pages (General/Projects/Diagnostics/About), which the
      // "general" (default) and "diagnostics" page ids in these scenarios
      // now render for real instead of a placeholder — a bootstrap this
      // thin would otherwise throw reading `.settings.theme` etc.
      settings: { version: 1 as const, projects: [] as string[], pinnedSessions: [] as never[] },
      workspace: hasProject ? { path: '/test/project', trusted: true } : { trusted: false },
      runtimes: [] as never[],
      diagnostics: [] as never[],
      versions: { app: 'test-app', electron: 'test-electron' },
    },
    connected: connected && !sessionLoading,
    sessionLoading,
    running,
    settingsSnapshotEvent,
    // Task 18 pages: `McpServers`/`Skills` call these on MOUNT (not just on
    // a button click), so — unlike the write-side commands below, which
    // only fire on an explicit click these scenarios never make — they must
    // exist here or selecting either page throws through the render.
    mcpServersEvent: null,
    skillsEvent: null,
    refreshMcpServers: noopAsyncVoid,
    refreshSkills: noopAsyncVoid,
    restartBridge,
    refreshSettingsSnapshot,
    refreshDiagnostics: noopAsyncArray,
    copyDiagnostics: noopAsyncVoid,
    exportDiagnostics: noopAsyncNull,
    addProject: noopAsyncNull,
    removeProject: noopAsyncVoid,
    activateProject: noopAsyncNull,
    setThemePreference: noopAsyncVoid,
    sessionRuntimeStatus: () => undefined,
    // Write-side commands for Task 18's pages — none of these scenarios
    // click a save/add/remove button on them, but they are here so a future
    // scenario that does doesn't have to rediscover this same crash.
    updateEngineSettings: noopAsyncVoid,
    updatePermissionRules: noopAsyncVoid,
    setDefaultPermissionMode: noopAsyncVoid,
    updateWorkspaceDirectories: noopAsyncVoid,
    upsertMcpServer: noopAsyncVoid,
    removeMcpServer: noopAsyncVoid,
    runSlashCommand: noopAsyncVoid,
  };

  useEffect(() => {
    window.__settingsScreenTest = {
      selectPage: (id: string) => {
        (document.querySelector(`[data-nav-page="${id}"]`) as HTMLButtonElement | null)?.click();
      },
      setHasProject,
      setRunning,
      setConnected,
      setSessionLoading,
      setRestartShouldFail,
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
      openSettings: () => setOpen(true),
      closeSettings: () => setOpen(false),
      focusOpener: () => (document.getElementById('opener') as HTMLButtonElement | null)?.focus(),
      state: () => ({
        hasLayerSwitcher: Boolean(document.querySelector('[data-testid="layer-switcher"]')),
        userDisabled: (document.querySelector('[data-layer="user"]') as HTMLButtonElement | null)?.disabled ?? null,
        projectDisabled: (document.querySelector('[data-layer="project"]') as HTMLButtonElement | null)?.disabled ?? null,
        localDisabled: (document.querySelector('[data-layer="local"]') as HTMLButtonElement | null)?.disabled ?? null,
        layerSwitcherReasonText: document.querySelector('[data-testid="layer-switcher-disabled-reason"]')?.textContent ?? null,
        hasBanner: Boolean(document.querySelector('[data-testid="settings-pending-banner"]')),
        bannerText: document.querySelector('[data-testid="settings-pending-banner"] span')?.textContent ?? null,
        restartButtonDisabled: (document.querySelector('[data-testid="settings-pending-banner"] button') as HTMLButtonElement | null)?.disabled ?? null,
        restartDisabledReasonText: document.querySelector('[data-testid="restart-disabled-reason"]')?.textContent ?? null,
        hasRestartError: Boolean(document.querySelector('[data-testid="settings-restart-error"]')),
        restartErrorText: document.querySelector('[data-testid="settings-restart-error"]')?.textContent ?? null,
        placeholderKind: document.querySelector('[data-testid="page-placeholder"]')?.getAttribute('data-placeholder-kind') ?? null,
        hasSnapshotError: Boolean(document.querySelector('[data-testid="settings-snapshot-error"]')),
        activeElementAriaLabel: document.activeElement instanceof HTMLElement ? document.activeElement.getAttribute('aria-label') : null,
        activeElementId: document.activeElement instanceof HTMLElement ? document.activeElement.id : null,
        dialogPresent: Boolean(document.querySelector('[role="dialog"]')),
        restartCalls,
        refreshCalls,
        closeCalls,
      }),
    };
    return () => { delete window.__settingsScreenTest; };
  }, [restartCalls, refreshCalls, closeCalls]);

  return (
    <Theme.Provider value={tokens('light')}>
      <div>
        <button type="button" id="opener">Open settings</button>
        {open && (
          <SettingsScreen
            bridge={bridge as never}
            theme="light"
            onTheme={() => {}}
            onClose={() => { setCloseCalls((n) => n + 1); setOpen(false); }}
          />
        )}
      </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __settingsScreenTest?: {
      selectPage(id: string): void;
      setHasProject(value: boolean): void;
      setRunning(value: boolean): void;
      setConnected(value: boolean): void;
      setSessionLoading(value: boolean): void;
      setRestartShouldFail(message: string | null): void;
      setSnapshot(effective: Record<string, unknown>, active: Record<string, unknown>): void;
      setMalformedSnapshot(): void;
      clickRestart(): void;
      close(): void;
      openSettings(): void;
      closeSettings(): void;
      focusOpener(): void;
      state(): {
        hasLayerSwitcher: boolean;
        userDisabled: boolean | null;
        projectDisabled: boolean | null;
        localDisabled: boolean | null;
        layerSwitcherReasonText: string | null;
        hasBanner: boolean;
        bannerText: string | null;
        restartButtonDisabled: boolean | null;
        restartDisabledReasonText: string | null;
        hasRestartError: boolean;
        restartErrorText: string | null;
        placeholderKind: string | null;
        hasSnapshotError: boolean;
        activeElementAriaLabel: string | null;
        activeElementId: string | null;
        dialogPresent: boolean;
        restartCalls: number;
        refreshCalls: number;
        closeCalls: number;
      };
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
