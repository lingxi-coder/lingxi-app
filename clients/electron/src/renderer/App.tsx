import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import { useBridge } from './bridge/useBridge';
import {
  BetaComposer,
  BetaSidebar,
  BetaTopBar,
  ErrorBanner,
} from './components/BetaDesktop';
import { DesktopCommandPalette } from './components/DesktopCommandPalette';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
import { PlanTasks } from './components/PlanTasks';
import { SettingsScreen } from './components/settings/SettingsScreen';
import { Stage } from './components/Stage';
import { Theme } from './theme/ThemeContext';
import { tokens, watchThemePreference, type ThemeMode } from './theme/tokens';
import { RuntimeCenterInspector, RuntimeCenterOverview } from './components/RuntimeCenter';

export function App() {
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [settingsRoute, setSettingsRoute] = useState<SettingsRoute | null>(null);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
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
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isCommandPaletteShortcut(event)) return;
      event.preventDefault();
      setCommandPaletteOpen((open) => !open);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
  const changeTheme = (value: ThemeMode) => {
    setTheme(value);
    void bridge.setThemePreference(value).catch(() => undefined);
  };
  const openSettings = (route?: Pick<SettingsRoute, 'pageId' | 'providerId' | 'pendingModelReference'>) => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setCommandPaletteOpen(false);
    setSettingsRoute({ ...route, restoreFocus: () => opener?.focus() });
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
        className="desktop-shell"
        data-screen-label="LingXi Code Desktop Beta"
        style={{ width: '100vw', height: '100vh', overflow: 'hidden', display: 'flex', position: 'relative', background: palette.windowBg, color: palette.text }}
      >
        <SettingsBackground active={settingsRoute !== null}>
          <BetaSidebar
            bridge={bridge}
            onOpenSettings={() => openSettings()}
          />

          <main className="desktop-main" style={{ position: 'relative', flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
          <BetaTopBar
            bridge={bridge}
            runtimeCenterOpen={bridge.runtimeCenter.overviewOpen}
            onToggleRuntimeCenter={() => bridge.setRuntimeCenterOverviewOpen(!bridge.runtimeCenter.overviewOpen)}
            theme={theme}
            onTheme={changeTheme}
            onOpenSettingsPage={(pageId) => openSettings(pageId ? { pageId } : undefined)}
            onOpenCommandPalette={() => setCommandPaletteOpen(true)}
          />
          {!bridge.sessionLoading && <RuntimeCenterOverview bridge={bridge} />}
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
                  openSettings();
                }}
                onOpenSettingsPage={(pageId) => openSettings({ pageId })}
                onSetTheme={changeTheme}
                onOpenProviderSettings={(providerId, modelReference, restoreFocus) => setSettingsRoute({ pageId: 'provider-credentials', providerId, pendingModelReference: modelReference, restoreFocus })}
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
          {!bridge.sessionLoading && <RuntimeCenterInspector bridge={bridge} />}
        </SettingsBackground>

        <DesktopCommandPalette
          open={commandPaletteOpen}
          bridge={bridge}
          theme={theme}
          onClose={() => setCommandPaletteOpen(false)}
          onOpenSettingsPage={(pageId) => openSettings(pageId ? { pageId } : undefined)}
        />

        {settingsRoute && (
          <SettingsScreen
            bridge={bridge}
            theme={theme}
            onTheme={changeTheme}
            initialPageId={settingsRoute.pageId}
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

export function isCommandPaletteShortcut(
  event: Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'key' | 'metaKey' | 'shiftKey'>,
): boolean {
  return !event.altKey
    && !event.shiftKey
    && (event.metaKey || event.ctrlKey)
    && event.key.toLowerCase() === 'k';
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
  pageId?: string;
  providerId?: string;
  pendingModelReference?: string;
  restoreFocus?: () => void;
}
