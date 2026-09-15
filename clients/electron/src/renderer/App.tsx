import { conversationPlans } from './bridge/planDocuments';
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import { useBridge } from './bridge/useBridge';
import {
  BetaComposer,
  BetaSidebar,
  BetaTopBar,
  ErrorBanner,
} from './components/BetaDesktop';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
import { SettingsScreen } from './components/settings/SettingsScreen';
import { ScheduledTasks } from './components/ScheduledTasks';
import { Stage } from './components/Stage';
import { Theme } from './theme/ThemeContext';
import { tokens, watchThemePreference, type ThemeMode } from './theme/tokens';
import { RuntimeCenterInspector, RuntimeCenterOverview } from './components/RuntimeCenter';
import './components/RuntimeCenter.css';
import './components/TranscriptAgents.css';

export function App() {
  const [page, setPage] = useState<'chat' | 'scheduled'>('chat');
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [settingsRoute, setSettingsRoute] = useState<SettingsRoute | null>(null);
  const palette = useMemo(() => tokens(theme === 'dark'), [theme]);
  const bridge = useBridge();
  const workspace = bridge.bootstrap?.workspace;
  // Stage's agent-anchor cache compares this prop BY REFERENCE, so an inline
  // `Object.values(...)` (or a bare `[]` literal) would hand it a new identity on
  // every render, invalidating the anchor map and, through it, the transcript
  // `rows` memo — turning a memoized transcript into a full re-walk per render.
  const stageAgents = useMemo(
    () => bridge.sessionLoading ? [] : Object.values(bridge.runtimeCenter.agents),
    [bridge.sessionLoading, bridge.runtimeCenter.agents],
  );
  const planCalls = useMemo(() => conversationPlans(bridge.conversation.items, bridge.runtimeCenter.submittedPlanState.calls), [bridge.runtimeCenter.submittedPlanState.calls, bridge.conversation.items]);
  const planBridge = { ...bridge, runtimeCenter: { ...bridge.runtimeCenter, submittedPlan: planCalls.at(-1) ?? bridge.runtimeCenter.submittedPlan, submittedPlanState: { ...bridge.runtimeCenter.submittedPlanState, calls: planCalls } } };
  const providerConfigured = Boolean(bridge.bootstrap?.providerCredentials?.some((entry) => entry.configured));
  const ready = Boolean(
    !bridge.loading
    && bridge.hosted
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
  const openSettings = (route?: Pick<SettingsRoute, 'pageId' | 'providerId' | 'pendingModelReference'>) => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
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
            scheduled={page === 'scheduled'}
            onOpenScheduled={() => setPage('scheduled')}
            onOpenChat={() => setPage('chat')}
            onOpenSettings={() => openSettings()}
          />

          <main className="desktop-main" style={{ position: 'relative', flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
          <div style={{ display: page === 'scheduled' ? 'contents' : 'none' }}>
          <ScheduledTasks key={bridge.activeSession?.sessionId ?? workspace?.path ?? 'no-project'} bridge={bridge} visible={page === 'scheduled'} />
          </div>
          <div style={{ display: page === 'chat' ? 'contents' : 'none' }}>
          <BetaTopBar
            bridge={bridge}
            runtimeCenterOpen={bridge.runtimeCenter.overviewOpen}
            onToggleRuntimeCenter={() => bridge.setRuntimeCenterOverviewOpen(!bridge.runtimeCenter.overviewOpen)}
          />
          {!bridge.sessionLoading && <RuntimeCenterOverview bridge={planBridge} />}
          <ErrorBanner bridge={bridge} />
              {/* The transcript keeps all remaining height; Todos live in Summary. */}
              <Stage
                submittedPlans={bridge.sessionLoading ? [] : planCalls}
                onOpenPlan={(id) => bridge.openRuntimeItem({ kind: 'plan-document', id })}
                liveItems={bridge.sessionLoading ? [] : bridge.conversation.items}
                running={!bridge.sessionLoading && bridge.running}
                agents={stageAgents}
                welcomeProject={(bridge.activeSession?.projectPath ?? bridge.bootstrap?.settings.activeProject ?? workspace?.path)?.split(/[\\/]/).filter(Boolean).at(-1) ?? ''}
                onOpenAgent={(agentId) => bridge.openRuntimeItem({ kind: 'agent', id: agentId })}
                collapseThoughtsByDefault={bridge.bootstrap?.settings.collapseThoughtsByDefault ?? true}
                emptyMessage={emptyMessage}
                // Item ids restart at `i1` in every session; the Stage's
                // collapse map is scoped by this and dropped when it changes.
                sessionKey={bridge.conversation.sessionKey}
                foldedItemIds={bridge.sessionLoading ? [] : bridge.conversation.foldedItemIds}
              />
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

          </div>

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
          {page === 'chat' && !bridge.sessionLoading && <RuntimeCenterInspector bridge={planBridge} />}
        </SettingsBackground>

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
