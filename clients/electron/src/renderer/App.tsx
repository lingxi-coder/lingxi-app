import { useEffect, useMemo, useState } from 'react';

import { useBridge } from './bridge/useBridge';
import {
  BetaComposer,
  BetaSettings,
  BetaSidebar,
  BetaTasks,
  BetaTopBar,
  ErrorBanner,
  SetupCard,
} from './components/BetaDesktop';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
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
    && bridge.connected,
  );
  useEffect(() => {
    const savedTheme = bridge.bootstrap?.settings.theme;
    if (savedTheme) setTheme(savedTheme);
  }, [bridge.bootstrap?.settings.theme]);
  const changeTheme = (value: ThemeMode) => {
    setTheme(value);
    void bridge.setThemePreference(value).catch(() => undefined);
  };

  const emptyMessage = bridge.desktop.activeSessionId
    ? 'This session has no messages yet. Ask LingXi to inspect the workspace.'
    : 'Create a session and ask LingXi to inspect, explain, or change this workspace.';

  return (
    <Theme.Provider value={palette}>
      <div
        data-screen-label="LingXi Code Desktop Beta"
        style={{ width: '100vw', height: '100vh', overflow: 'hidden', display: 'flex', position: 'relative', background: palette.windowBg, color: palette.text }}
      >
        <div aria-hidden="true" style={{ position: 'absolute', top: 16, left: 17, zIndex: 30, display: 'flex', gap: 8 }}>
          {['#ff5f57', '#febc2e', '#28c840'].map((color) => <span key={color} style={{ width: 12, height: 12, borderRadius: '50%', background: color, border: '0.5px solid rgba(0,0,0,.12)' }} />)}
        </div>

        <BetaSidebar bridge={bridge} onOpenSettings={() => setSettingsOpen(true)} />

        <main style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
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
          ) : ready ? (
            <>
              <Stage liveItems={bridge.conversation.items} running={bridge.running} emptyMessage={emptyMessage} />
              <BetaComposer bridge={bridge} ready={ready} />
            </>
          ) : (
            <SetupCard bridge={bridge} />
          )}
        </main>

        {tasksOpen && <BetaTasks bridge={bridge} onClose={() => setTasksOpen(false)} />}

        <PermissionPrompt
          request={bridge.pendingPermission}
          onApprove={(requestId, response) => { void bridge.approve(requestId, response).catch(() => undefined); }}
          onDeny={(requestId) => { void bridge.deny(requestId).catch(() => undefined); }}
        />

        <ComputerAccessPrompt
          request={bridge.pendingComputerAccess}
          onSubmit={(requestId, response) => { void bridge.approveComputerAccess(requestId, response).catch(() => undefined); }}
          onDeny={(requestId) => { void bridge.denyComputerAccess(requestId).catch(() => undefined); }}
          onOpenSystemSettings={(pane) => { void bridge.openSystemSettings(pane).catch(() => undefined); }}
        />

        <AskUserQuestionPrompt
          request={bridge.pendingAskUserQuestion}
          onSubmit={(requestId, answers) => { void bridge.answerAskUserQuestion(requestId, answers).catch(() => undefined); }}
          onCancel={(requestId) => { void bridge.cancelAskUserQuestion(requestId).catch(() => undefined); }}
        />

        {settingsOpen && (
          <BetaSettings bridge={bridge} theme={theme} onTheme={changeTheme} onClose={() => setSettingsOpen(false)} />
        )}
      </div>
    </Theme.Provider>
  );
}
