import { useCallback, useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { BetaTopBar } from '../../src/renderer/components/BetaDesktop';
import { RuntimeCenterInspector, RuntimeCenterOverview } from '../../src/renderer/components/RuntimeCenter';
import { emptyConversation } from '../../src/renderer/bridge/conversation';
import { emptyDesktopState } from '../../src/renderer/bridge/desktopState';
import {
  closeRuntimeCenterItem, emptyRuntimeCenterState, openRuntimeCenterItem,
  setRuntimeCenterOverviewOpen, setRuntimeInspectorOpen,
  type RuntimeCenterItemRef,
} from '../../src/renderer/bridge/runtimeCenterState';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/RuntimeCenter.css';

const idle = async () => undefined;
const todos = [
  { id: '1', subject: 'Inspect the existing desktop layout', state: 'completed' as const },
  { id: '2', subject: 'Align the summary and right panel', state: 'in_progress' as const },
  { id: '3', subject: 'Verify keyboard and window behavior', state: 'pending' as const },
];
function initialCenter(empty: boolean) {
  const center = emptyRuntimeCenterState();
  if (empty) return center;
  return {
    ...center,
    agents: {
      'agent-1': { agent_id: 'agent-1', name: 'Layout review', agent_type: 'reviewer', status: 'completed', latest_activity: 'Reviewed toolbar spacing', model: 'Default', parent_session_id: 'session-a' },
    },
    plan: todos,
    resources: [
      { id: 'r1', kind: 'file' as const, name: 'desktop-reference.png', path: '/workspace/desktop-reference.png' },
      { id: 'r2', kind: 'file' as const, name: 'requirements.md', path: '/workspace/requirements.md' },
      { id: 'r3', kind: 'file' as const, name: 'interaction-notes.md', path: '/workspace/interaction-notes.md' },
      { id: 'r4', kind: 'file' as const, name: 'acceptance.md', path: '/workspace/acceptance.md' },
    ],
    submittedPlan: { id: 'plan-1', content: '# Desktop workspace\n\nBring the desktop toolbar and task details together in a focused workspace.\n\n## Implementation\n\n- Keep the summary pinned while reading the conversation.\n- Open detailed information in the right panel.\n- Separate the task checklist from the submitted plan.\n\n## Verification\n\nCheck the light and dark themes, keyboard navigation, and narrow windows.', status: 'approved' as const },
  } as ReturnType<typeof emptyRuntimeCenterState>;
}

function Fixture() {
  const [theme, setTheme] = useState<'dark' | 'light'>('light');
  const [sessionKey, setSessionKey] = useState('session-a');
  const [empty, setEmpty] = useState(false);
  const [center, setCenter] = useState(() => initialCenter(false));
  const palette = tokens(theme === 'dark');
  useEffect(() => {
    const reset = (event: Event) => {
      const options = (event as CustomEvent).detail;
      if (options.theme) setTheme(options.theme);
      if (options.sessionKey) {
        setSessionKey(options.sessionKey);
        setEmpty(Boolean(options.empty));
        setCenter(initialCenter(Boolean(options.empty)));
      }
    };
    window.addEventListener('fixture-reset', reset);
    return () => window.removeEventListener('fixture-reset', reset);
  }, []);
  const openRuntimeItem = useCallback((item: RuntimeCenterItemRef) => setCenter((current) => openRuntimeCenterItem(current, item)), []);
  const closeRuntimeItem = useCallback((item: RuntimeCenterItemRef) => setCenter((current) => closeRuntimeCenterItem(current, item)), []);
  const setOverview = useCallback((open: boolean) => setCenter((current) => setRuntimeCenterOverviewOpen(current, open)), []);
  const setInspector = useCallback((open: boolean) => setCenter((current) => setRuntimeInspectorOpen(current, open)), []);
  const desktop = useMemo(() => emptyDesktopState(), []);
  const bridge = {
    activeSession: { projectPath: '/Users/tester/Projects/LingXi-Next', sessionId: sessionKey },
    bootstrap: { workspace: { path: '/Users/tester/Projects/LingXi-Next', trusted: true } },
    usage: { inputTokens: 18_420, outputTokens: 3_184, cacheReadTokens: 0, cacheCreationTokens: 0 },
    conversation: {
      ...emptyConversation(), sessionKey, plan: empty ? [] : todos,
      summaries: [
        { id: 'summary-1', content: '## Provider routing\n\nPreserve the selected provider when the session resumes.', messagesBefore: 42, messagesAfter: 9, bytesSaved: 38912 },
        { id: 'summary-2', content: '## Desktop polish\n\nKeep the workspace quiet and focused.', messagesBefore: 31, messagesAfter: 7, bytesSaved: 24576 },
      ],
    },
    runtimeCenter: center, desktop,
    openRuntimeItem, closeRuntimeItem, setRuntimeCenterOverviewOpen: setOverview, setRuntimeInspectorOpen: setInspector,
    refreshTasks: idle, refreshSessionAgents: idle, loadSessionAgentTranscript: idle, taskOutput: idle,
    previewWorkspaceFile: async () => ({ content: 'Reference content', path: '/workspace/requirements.md', kind: 'text' }),
  };
  return (
    <Theme.Provider value={palette}>
      <div className="desktop-shell" style={{ width: '100vw', height: '100vh', position: 'relative', display: 'flex', overflow: 'hidden', background: palette.stageBg, color: palette.text }}>
        <main className="desktop-main" style={{ minWidth: 0, position: 'relative', flex: 1, display: 'flex', flexDirection: 'column' }}>
          <BetaTopBar bridge={bridge as never} runtimeCenterOpen={center.overviewOpen} onToggleRuntimeCenter={() => setOverview(!center.overviewOpen)} />
          <RuntimeCenterOverview bridge={bridge as never} />
          <div data-fixture-chat="true" tabIndex={0} style={{ flex: 1, padding: '60px 40px', overflow: 'auto', fontSize: 14, lineHeight: 1.7 }}>
            <div style={{ maxWidth: 640, margin: '0 auto' }}>
              <p style={{ display: 'inline-block', borderRadius: 20, background: palette.surfaceHover, padding: '12px 18px', marginBottom: 30 }}>Align the desktop topbar with Codex.</p>
              <p>The summary keeps subagents, todos, resources, and the submitted plan close at hand.</p>
              <p style={{ color: palette.text3 }}>Select an item to see its details in the right panel.</p>
            </div>
          </div>
          <div style={{ margin: '16px 28px', padding: '18px 20px', border: `1px solid ${palette.border}`, borderRadius: 24, color: palette.text3 }}>Ask a follow-up…</div>
        </main>
        <RuntimeCenterInspector bridge={bridge as never} />
      </div>
    </Theme.Provider>
  );
}

createRoot(document.getElementById('root')!).render(<Fixture />);
