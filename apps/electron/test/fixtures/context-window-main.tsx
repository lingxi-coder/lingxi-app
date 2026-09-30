import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { emptyConversation, reduceEvent } from '../../src/renderer/bridge/conversation';
import type { AgentEvent } from '@lingxi/bridge-client';
import { ContextWindow } from '../../src/renderer/components/ContextWindow';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

type Live = { inputTokens: number; outputTokens: number; cacheReadTokens: number; cacheCreationTokens: number };
const live: Live = { inputTokens: 79000, outputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 };

declare global {
  interface Window { sendUsageEvent(event: AgentEvent): void }
}

function Harness() {
  const [state, setState] = useState<ReturnType<typeof emptyConversation>>(() => ({ ...emptyConversation(), usage: live }));
  useEffect(() => { window.sendUsageEvent = event => setState(previous => reduceEvent(previous, event)); }, []);
  return <ContextWindow capacity={475000} usage={state.usage} />;
}

createRoot(document.getElementById('root')!).render(
  <Theme.Provider value={tokens(false)}>
    <main style={{ padding: 180, display: 'flex', gap: 24, alignItems: 'center' }}>
      <Harness />
      <button id="other" type="button">Other control</button>
    </main>
  </Theme.Provider>,
);
