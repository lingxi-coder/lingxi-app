import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import { useBridge } from './bridge/useBridge';
import {
  BetaComposer,
  BetaSidebar,
  BetaTasks,
  BetaTopBar,
  ErrorBanner,
} from './components/BetaDesktop';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
import { PlanTasks } from './components/PlanTasks';
import { SettingsScreen } from './components/settings/SettingsScreen';
import { Stage } from './components/Stage';
import { Theme } from './theme/ThemeContext';
import { tokens, watchThemePreference, type ThemeMode } from './theme/tokens';

export function App() {
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [tasksOpen, setTasksOpen] = useState(false);
  const [settingsRoute, setSettingsRoute] = useState<SettingsRoute | null>(null);
  const palette = useMemo(() => tokens(theme === 'dark'), [theme]);
  const bridge = useBridge();
  const workspace = bridge.bootstrap?.workspace;
  const providerConfigured = Boolean(bridge.bootstrap?.providerCredentials?.some((entry) => entry.configured));
  const ready = Boolean(
    bridge.hosted
    && workspace?.path
    && workspace.trusted
    && providerConfigured
    && bridge.connected
    && !bridge.sessionLoading,
  );
  useEffect(() => {
    // `'system'` isn't a third palette (see `ThemeMode`) — it's a preference
    // that resolves to one of the two, and keeps following the OS via the
    // media query's `change` event for as long as this effect is mounted.
    const preference = bridge.bootstrap?.settings.theme;
    return watchThemePreference(preference, window.matchMedia('(prefers-color-scheme: dark)'), setTheme);
  }, [bridge.bootstrap?.settings.theme]);
  const changeTheme = (value: ThemeMode) => {
    setTheme(value);
    void bridge.setThemePreference(value).catch(() => undefined);
  };

  const emptyMessage = bridge.sessionLoading
    ? 'Loading session…'
    : !workspace?.path
    ? 'Add a project from the sidebar to start a session.'
    : bridge.activeSession
      ? 'This session has no messages yet. Ask LingXi to inspect the project.'
      : 'Create a session and ask LingXi to inspect, explain, or change this project.';

  return (
    <Theme.Provider value={palette}>
      <div
        data-screen-label="LingXi Code Desktop Beta"
        style={{ width: '100vw', height: '100vh', overflow: 'hidden', display: 'flex', position: 'relative', background: palette.windowBg, color: palette.text }}
      >
        <SettingsBackground active={settingsRoute !== null}>
          <BetaSidebar
            bridge={bridge}
            onOpenSettings={() => {
              // Read the focused element HERE, in the event handler, not in the
              // dialog's mount effect: `SettingsBackground` applies `inert` in a
              // LAYOUT effect, which runs before the dialog's passive mount
              // effect, and the browser's unfocusing steps have already moved
              // focus to <body> by then. Without this the gear path restores
              // focus to a non-tabbable <body> and a keyboard user restarts
              // from the top of the app. The model-picker path already threads
              // `restoreFocus`; this gives the gear the same contract.
              const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
              setSettingsRoute({ restoreFocus: () => opener?.focus() });
            }}
          />

          <main style={{ position: 'relative', flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
          <BetaTopBar
            bridge={bridge}
            tasksOpen={tasksOpen}
            onToggleTasks={() => setTasksOpen((value) => !value)}
            theme={theme}
            onTheme={changeTheme}
          />
          <ErrorBanner bridge={bridge} />
          {bridge.loading ? (
            <div role="status" style={{ flex: 1, display: 'grid', placeItems: 'center', color: palette.text3, fontSize: 13 }}>Loading secure desktop state…</div>
          ) : (
            <>
              {/*
                Three flex siblings in a column: the Stage takes the remaining
                height, the plan strip and the composer keep theirs. Making the
                plan a SIBLING rather than an overlay is the point — it shrinks
                the scroll viewport instead of covering the newest tool output.
              */}
              <Stage
                liveItems={bridge.sessionLoading ? [] : bridge.conversation.items}
                running={!bridge.sessionLoading && bridge.running}
                emptyMessage={emptyMessage}
                // Item ids restart at `i1` in every session; the Stage's
                // collapse map is scoped by this and dropped when it changes.
                sessionKey={bridge.conversation.sessionKey}
              />
              <PlanTasks tasks={bridge.sessionLoading ? [] : bridge.conversation.plan} />
              <BetaComposer
                bridge={bridge}
                ready={ready}
                onOpenSettings={() => {
                  // `/config` from the composer opens the same full-page
                  // surface the gear does, so it owes the same focus contract:
                  // capture the opener HERE, before `SettingsBackground`'s
                  // layout effect marks the tree `inert` and the browser has
                  // already moved focus to <body>. See the sidebar handler.
                  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
                  setSettingsRoute({ restoreFocus: () => opener?.focus() });
                }}
                onSetTheme={changeTheme}
                onOpenProviderSettings={(providerId, modelReference, restoreFocus) => setSettingsRoute({ providerId, pendingModelReference: modelReference, restoreFocus })}
              />
            </>
          )}

          <PermissionPrompt
            request={bridge.sessionLoading ? null : bridge.pendingPermission}
            onApprove={(requestId, response) => { void bridge.approve(requestId, response).catch(() => undefined); }}
            onDeny={(requestId) => { void bridge.deny(requestId).catch(() => undefined); }}
          />

          <ComputerAccessPrompt
            request={bridge.sessionLoading ? null : bridge.pendingComputerAccess}
            onSubmit={(requestId, response) => { void bridge.approveComputerAccess(requestId, response).catch(() => undefined); }}
            onDeny={(requestId) => { void bridge.denyComputerAccess(requestId).catch(() => undefined); }}
            onOpenSystemSettings={(pane) => { void bridge.openSystemSettings(pane).catch(() => undefined); }}
          />

          <AskUserQuestionPrompt
            request={bridge.sessionLoading ? null : bridge.pendingAskUserQuestion}
            onSubmit={(requestId, answers) => { void bridge.answerAskUserQuestion(requestId, answers).catch(() => undefined); }}
            onCancel={(requestId) => { void bridge.cancelAskUserQuestion(requestId).catch(() => undefined); }}
          />
          </main>

          {tasksOpen && !bridge.sessionLoading && <BetaTasks bridge={bridge} onClose={() => setTasksOpen(false)} />}
        </SettingsBackground>

        {settingsRoute && (
          <SettingsScreen
            bridge={bridge}
            theme={theme}
            onTheme={changeTheme}
            initialProviderId={settingsRoute.providerId}
            pendingModelReference={settingsRoute.pendingModelReference}
            onClose={() => {
              const restoreFocus = settingsRoute.restoreFocus;
              setSettingsRoute(null);
              window.requestAnimationFrame(() => restoreFocus?.());
            }}
          />
        )}
      </div>
    </Theme.Provider>
  );
}

export function setSettingsBackgroundInert(
  element: Pick<HTMLElement, 'setAttribute' | 'removeAttribute'>,
  active: boolean,
): void {
  if (active) {
    element.setAttribute('inert', '');
    element.setAttribute('aria-hidden', 'true');
  } else {
    element.removeAttribute('inert');
    element.removeAttribute('aria-hidden');
  }
}

export function SettingsBackground({ active, children }: { active: boolean; children: ReactNode }) {
  const backgroundRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (backgroundRef.current) setSettingsBackgroundInert(backgroundRef.current, active);
  }, [active]);
  return <div ref={backgroundRef} aria-hidden={active ? 'true' : undefined} style={{ display: 'contents' }}>{children}</div>;
}

export interface SettingsRoute {
  providerId?: string;
  pendingModelReference?: string;
  restoreFocus?: () => void;
}
