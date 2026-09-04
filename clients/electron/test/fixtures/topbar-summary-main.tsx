import { useState } from 'react';
import { createRoot } from 'react-dom/client';

import { BetaTopBar } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const bridge = {
  activeSession: { projectPath: '/Users/tester/Projects/LingXi-Next', sessionId: 'session-a' },
  bootstrap: { workspace: { path: '/Users/tester/Projects/LingXi-Next', trusted: true } },
  usage: { inputTokens: 18_420, outputTokens: 3_184, cacheReadTokens: 0, cacheCreationTokens: 0 },
  conversation: {
    sessionKey: 'session-a',
    summaries: [
      {
        id: 'summary-1',
        content: '## Provider routing\n\nThe session keeps its selected provider and reloads the matching API key when the model changes.\n\n- Preserve the session model on resume\n- Resolve credentials before forwarding the switch',
        messagesBefore: 42,
        messagesAfter: 9,
        bytesSaved: 38_912,
      },
      {
        id: 'summary-2',
        content: '## Desktop polish\n\nThe topbar now uses quiet, transparent icon controls and keeps operational status out of the primary chrome.\n\n### Current direction\n\nShow compacted context in a focused two-pane summary viewer.',
        messagesBefore: 31,
        messagesAfter: 7,
        bytesSaved: 24_576,
      },
    ],
  },
};

function Fixture() {
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [runtimeCenterOpen, setRuntimeCenterOpen] = useState(false);
  const palette = tokens(theme === 'dark');
  return (
    <Theme.Provider value={palette}>
      <main style={{ width: '100vw', height: '100vh', overflow: 'hidden', background: palette.stageBg, color: palette.text }}>
        <BetaTopBar
          bridge={bridge as never}
          runtimeCenterOpen={runtimeCenterOpen}
          onToggleRuntimeCenter={() => setRuntimeCenterOpen((open) => !open)}
          theme={theme}
          onTheme={setTheme}
        />
        <div style={{ width: 'min(680px, calc(100% - 80px))', margin: '68px auto 0', display: 'grid', gap: 18, opacity: .52 }}>
          <div style={{ width: '42%', height: 14, borderRadius: 7, background: palette.surfaceHover }} />
          <div style={{ width: '100%', height: 1, background: palette.border }} />
          <div style={{ width: '86%', height: 11, borderRadius: 6, background: palette.surfaceHover }} />
          <div style={{ width: '72%', height: 11, borderRadius: 6, background: palette.surfaceHover }} />
        </div>
      </main>
    </Theme.Provider>
  );
}

createRoot(document.getElementById('root')!).render(<Fixture />);
