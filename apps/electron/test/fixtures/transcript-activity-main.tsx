import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import { emptyConversation, reduceEvent } from '../../src/renderer/bridge/conversation';
import { emptyDesktopState, reduceDesktopEvent } from '../../src/renderer/bridge/desktopState';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

let sequence = 0;
function initial(count: number) {
  return { ...emptyConversation(), sessionKey: `activity:${sequence++}`, running: true, nextId: 10000,
    items: Array.from({ length: count }, (_, i) => ({ type: 'narration' as const, id: `history-${i}`,
      role: 'assistant' as const, text: `History ${i}: the visible message must stay at the same reading position.` })),
  };
}
Object.assign(window, { lingxi: { modUi: { control: async () => ({ response: { tree: null } }),
  operation: async () => ({}), onFrame: () => () => {}, onInvalidate: () => () => {} } } });
function Fixture() {
  const [state, setState] = useState(() => initial(35));
  const [desktop, setDesktop] = useState(emptyDesktopState);
  Object.assign(window, { activityFixture: {
    reset(count: number) { flushSync(() => { setState(initial(count)); setDesktop(emptyDesktopState()); }); },
    event(event: Parameters<typeof reduceEvent>[1]) { flushSync(() => { setState(state => reduceEvent(state, event)); setDesktop(state => reduceDesktopEvent(state, event)); }); },
  } });
  return <Theme.Provider value={tokens(false)}><div style={{ width: 840, height: 420, display: 'flex', flexDirection: 'column' }}>
    <Stage liveItems={state.items} running={state.running} sessionKey={state.sessionKey} apiRetry={desktop.apiRetry} modUiSessionId="activity-fixture" />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
