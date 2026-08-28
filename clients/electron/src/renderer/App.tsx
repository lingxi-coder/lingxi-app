import { useEffect, useMemo, useState } from 'react';

import { useBridge } from './bridge/useBridge';
import {
  BetaComposer,
  BetaSettings,
  BetaSidebar,
  BetaTasks,
  BetaTopBar,
  ErrorBanner,
} from './components/BetaDesktop';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
import { PlanTasks } from './components/PlanTasks';
import { Stage } from './components/Stage';
import { Theme } from './theme/ThemeContext';
import { tokens, type ThemeMode } from './theme/tokens';

export function App() {
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [tasksOpen, setTasksOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
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
    const savedTheme = bridge.bootstrap?.settings.theme;
    if (savedTheme) setTheme(savedTheme);
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
        <BetaSidebar bridge={bridge} onOpenSettings={() => setSettingsOpen(true)} />

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
                onOpenSettings={() => setSettingsOpen(true)}
                onSetTheme={changeTheme}
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

        {settingsOpen && (
          <BetaSettings bridge={bridge} theme={theme} onTheme={changeTheme} onClose={() => setSettingsOpen(false)} />
        )}
      </div>
    </Theme.Provider>
  );
}
