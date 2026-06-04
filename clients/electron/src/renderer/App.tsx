import { useState, useMemo } from 'react';
import { Theme } from './theme/ThemeContext';
import { tokens, type ThemeMode } from './theme/tokens';
import { PROJECTS, MODELS, type Model, type RunItem } from './data';
import { Sidebar } from './components/Sidebar';
import { TopBar } from './components/TopBar';
import { Stage } from './components/Stage';
import { Composer } from './components/Composer';
import { RightPanel } from './components/RightPanel';
import { PermissionPrompt } from './components/PermissionPrompt';
import { SettingsPage } from './components/settings/SettingsPage';
import type { Mode } from './components/primitives';
import { useBridge } from './bridge/useBridge';

export function App() {
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const t = useMemo(() => tokens(theme === 'dark'), [theme]);
  const [mode, setMode] = useState<Mode>('code');
  const [activeRepo, setActiveRepo] = useState('lingxi-next');
  const [activeSession, setActiveSession] = useState('lxn-1');
  const [panelOpen, setPanelOpen] = useState(true);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [model, setModel] = useState<Model>(MODELS[0]);
  const [extraMessages, setExtraMessages] = useState<RunItem[]>([]);
  const appendMessage = (msg: RunItem) => setExtraMessages((m) => [...m, msg]);

  // Live bridge feed. When connected we render the real conversation; otherwise
  // the Stage falls back to the static mock RUN so the design preview still
  // works in a plain browser.
  const bridge = useBridge();
  const live = bridge.connected;

  const repo = PROJECTS.find((r) => r.id === activeRepo) || PROJECTS[0];

  return (
    <Theme.Provider value={t}>
      <div
        style={{
          width: '100vw', height: '100vh', background: t.appBg,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
          overflow: 'hidden', transition: 'background 0.3s ease',
        }}
        data-screen-label="灵犀 Code Desktop"
      >
        <div
          style={{
            width: '100%', height: '100%',
            background: t.windowBg, overflow: 'hidden',
            display: 'flex', position: 'relative',
            boxShadow: t.windowShadow,
          }}
        >
          {/* Traffic lights */}
          <div style={{ position: 'absolute', top: 14, left: 16, zIndex: 30, display: 'flex', gap: 8 }}>
            {['#ff5f57', '#febc2e', '#28c840'].map((c) => (
              <div key={c} style={{ width: 12, height: 12, borderRadius: '50%', background: c, border: '0.5px solid rgba(0,0,0,0.1)' }} />
            ))}
          </div>

          <Sidebar
            mode={mode}
            setMode={setMode}
            setActiveRepo={setActiveRepo}
            activeSession={activeSession}
            setActiveSession={setActiveSession}
            openSettings={() => setSettingsOpen(true)}
            collapsed={sidebarCollapsed}
            setCollapsed={setSidebarCollapsed}
          />

          {/* Main */}
          <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0, background: t.windowBg }}>
            <TopBar
              repo={repo}
              onTogglePanel={() => setPanelOpen(!panelOpen)}
              panelOpen={panelOpen}
              theme={theme}
              setTheme={setTheme}
              sidebarCollapsed={sidebarCollapsed}
              usage={live ? bridge.usage : null}
            />
            <Stage
              extraMessages={extraMessages}
              live={live}
              liveItems={bridge.conversation.items}
              running={bridge.running}
            />
            <Composer
              repo={repo}
              model={model}
              setModel={setModel}
              appendMessage={appendMessage}
              onSubmit={bridge.sendPrompt}
              onCancel={bridge.cancel}
              running={bridge.running}
            />
          </div>

          {panelOpen && <RightPanel onClose={() => setPanelOpen(false)} />}

          {/* Engine-parked permission requests: an allow-once / allow-always /
              deny prompt wired straight back over the bridge. Renders nothing
              when no request is pending. */}
          <PermissionPrompt
            request={bridge.pendingPermission}
            onApprove={bridge.approve}
            onDeny={bridge.deny}
          />
        </div>

        <SettingsPage open={settingsOpen} onClose={() => setSettingsOpen(false)} theme={theme} setTheme={setTheme} />
      </div>
    </Theme.Provider>
  );
}
