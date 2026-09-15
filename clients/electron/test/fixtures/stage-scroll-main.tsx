import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Real Stage, without an engine, credentials, persistence, or preload bridge.
const initialItems: RunItem[] = [
  ...Array.from({ length: 35 }, (_, index): RunItem => ({
    type: 'narration', id: `message-${index}`, role: 'assistant',
    text: `Message ${index}: A stable transcript line for scrolling regression coverage.`,
  })),
  { type: 'thinking', id: 'thinking-tail', text: 'Working', streamed: true },
];
function Fixture() {
  const [items, setItems] = useState(initialItems);
  Object.assign(window, { stageScrollFixture: {
    insert() {
      flushSync(() => setItems((current) => [
        ...current.slice(0, -1),
        { type: 'narration', id: `insert-${current.length}`, role: 'assistant', text: 'An intermediate streamed message while the final thinking status remains visible.' },
        current[current.length - 1]!,
      ]));
    },
  } });
  return <Theme.Provider value={tokens(false)}>
    <div id="scroll-ancestor" style={{ height: 300, overflowY: 'auto' }}>
      <div style={{ height: 420, display: 'flex', flexDirection: 'column' }}>
        <Stage liveItems={items} running sessionKey="scroll-fixture" />
      </div>
      <div style={{ height: 300 }} />
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
