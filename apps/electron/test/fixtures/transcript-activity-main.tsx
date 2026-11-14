import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import { emptyConversation, reduceEvent, type ConversationState } from '../../src/renderer/bridge/conversation';
import { emptyDesktopState, reduceDesktopEvent } from '../../src/renderer/bridge/desktopState';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

let sequence = 0;
function initial(count: number): ConversationState {
  return { ...emptyConversation(), sessionKey: `activity:${sequence++}`, running: true, nextId: 10000,
    items: Array.from({ length: count }, (_, i) => ({ type: 'narration' as const, id: `history-${i}`,
      role: 'assistant' as const, text: `History ${i}: the visible message must stay at the same reading position.` })),
  };
}
Object.assign(window, { lingxi: { modUi: { control: async () => ({ response: { tree: null } }),
  operation: async () => ({}), onFrame: () => () => {}, onInvalidate: () => () => {} } } });
function Fixture() {
  const [dark, setDark] = useState(false);
  const [state, setState] = useState(() => initial(35));
  const [desktop, setDesktop] = useState(emptyDesktopState);
  Object.assign(window, { activityFixture: {
    skill(dark: boolean) {
      const directory = 'Base directory for this skill: /skills/local-app\n';
      const resolution = 'Resolve relative references and assets from this directory; use their exact paths instead of searching the application home directory. Use read-only file tools for skill assets; keep project writes in the project directory.\n\n';
      flushSync(() => {
        setDark(dark);
        setState({ ...initial(0), running: false, items: [
          { type: 'tool', id: 'skill', tool: 'Skill', status: 'done',
            view: { verb: 'skill', label: 'Skill', title: 'Skill(local-app)', primary: 'local-app' },
            result: { headline: 'Launching skill: local-app' } },
          { type: 'narration', id: 'instructions', role: 'user', strong: true,
            text: directory + resolution + '# Local apps\n\nCreate an application project, publish it into this device’s Apps library, and verify the published application.\n\n## Choose the execution host and interfaces\n\nCurrent Local App instructions.' },
        ] });
      });
    },
    reset(count: number) { flushSync(() => { setState(initial(count)); setDesktop(emptyDesktopState()); }); },
    event(event: Parameters<typeof reduceEvent>[1]) { flushSync(() => { setState(state => reduceEvent(state, event)); setDesktop(state => reduceDesktopEvent(state, event)); }); },
  } });
  return <Theme.Provider value={tokens(dark)}><div style={{ width: 840, maxWidth: '100vw', height: 420, display: 'flex', flexDirection: 'column' }}>
    <Stage liveItems={state.items} running={state.running} sessionKey={state.sessionKey} apiRetry={desktop.apiRetry} modUiSessionId="activity-fixture" />
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
