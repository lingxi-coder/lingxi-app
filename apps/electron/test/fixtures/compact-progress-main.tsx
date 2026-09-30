import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import type { ClientEvent } from '@lingxi/bridge-client';

import { emptyConversation, reduceEvent, type ConversationState } from '../../src/renderer/bridge/conversation';
import { CompactionStatus } from '../../src/renderer/components/CompactionStatus';
import { TranscriptAgents } from '../../src/renderer/components/TranscriptAgents';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/TranscriptAgents.css';

// Only the real pure reducer and progress component are loaded. No desktop
// bridge, preload, config, session files, or credentials are used by this fixture.
function Fixture() {
  const [state, setState] = useState<ConversationState>(emptyConversation);
  const [mount, setMount] = useState(0);
  const palette = tokens(true);
  Object.assign(window, {
    compactFixture: {
      dispatch(event: ClientEvent) {
        flushSync(() => setState((current) => reduceEvent(current, event)));
      },
      reset() {
        flushSync(() => setState(emptyConversation()));
      },
      ageAndRemount() {
        flushSync(() => {
          setState((current) => ({
            ...current,
            items: current.items.map((item) => item.type === 'compaction'
              ? { ...item, startedAt: Date.now() - 90_000, phaseStartedAt: Date.now() - 90_000 }
              : item),
          }));
          setMount((current) => current + 1);
        });
      },
      remount() {
        flushSync(() => setMount((current) => current + 1));
      },
      state() { return state; },
    },
  });
  return (
    <Theme.Provider value={palette}>
      <main style={{ minHeight: '100vh', background: palette.stageBg, color: palette.text, padding: '64px 48px' }}>
        <div key={mount} style={{ maxWidth: 680, margin: '0 auto' }}>
          {state.items.map((item) => item.type === 'compaction'
            ? <CompactionStatus key={item.id} item={item} />
            : null)}
          <TranscriptAgents agents={[{
            agent_id: 'agent:compact-layout', name: 'fork-b24c', agent_type: 'explore', status: 'running', latest_activity: '4 tool uses · 0 tokens',
          }]} />
        </div>
      </main>
    </Theme.Provider>
  );
}

createRoot(document.getElementById('root')!).render(<Fixture />);
