import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// A stub for the one preload call this fixture exercises: without it the copy
// handler would fall through to `navigator.clipboard`, which a hidden test
// window has no reason to grant. Records what it was handed so the driver can
// assert what a click actually copied, and can be switched to fail so the
// rejection branch is exercised too.
let copyFails = false;
Object.assign(window, {
  lingxi: {
    copyText: async (text: string) => {
      if (copyFails) throw new Error('Clipboard unavailable');
      Object.assign(window, { copiedMessage: text });
    },
  },
});

// Real Stage, without an engine, credentials, or persistence.
const initialItems: RunItem[] = [
  ...Array.from({ length: 35 }, (_, index): RunItem => ({
    type: 'narration', id: `message-${index}`, role: 'assistant',
    text: `Message ${index}: A stable transcript line for scrolling regression coverage.`,
  })),
  { type: 'thinking', id: 'thinking-tail', text: 'Working', streamed: true },
];
function Fixture() {
  const [items, setItems] = useState(initialItems);
  const [thinkingVisible, setThinkingVisible] = useState(true);
  Object.assign(window, { stageScrollFixture: {
    thinking(visible: boolean) { flushSync(() => setThinkingVisible(visible)); },
    delivery(delivery: 'pending' | 'failed' | undefined, long = false) {
      flushSync(() => setItems([
        // `sentAt` present so the hover affordance renders BOTH halves of itself
        // (clock and copy button) — the layout guard is about the row's box, and
        // a missing clock would make it a different box.
        { type: 'narration', id: 'delivery-message', role: 'user', text: long ? 'A long follow-up message that wraps across several lines. '.repeat(10) : 'merge dev to main', delivery, sentAt: new Date(2026, 7, 26, 23, 35).getTime() },
        { type: 'narration', id: 'after-delivery', role: 'assistant', text: 'The following message must not move.' },
      ]));
    },
    insert() {
      flushSync(() => setItems((current) => [
        ...current.slice(0, -1),
        { type: 'narration', id: `insert-${current.length}`, role: 'assistant', text: 'An intermediate streamed message while the final thinking status remains visible.' },
        current[current.length - 1]!,
      ]));
    },
    // TWO prompts with distinct text: a single-row transcript cannot tell
    // "copied the right message" apart from "copied the only message".
    twoPrompts() {
      flushSync(() => setItems([
        { type: 'narration', id: 'first-prompt', role: 'user', text: 'merge dev to main', sentAt: new Date(2026, 7, 26, 23, 35).getTime() },
        { type: 'narration', id: 'second-prompt', role: 'user', text: 'ship the release notes', sentAt: new Date(2026, 7, 26, 23, 41).getTime() },
        { type: 'narration', id: 'after-prompts', role: 'assistant', text: 'Both prompts are on screen.' },
      ]));
    },
    failCopies(fail: boolean) { copyFails = fail; },
  } });
  return <Theme.Provider value={tokens(false)}>
    <div id="scroll-ancestor" style={{ height: 300, overflowY: 'auto' }}>
      <div style={{ height: 420, display: 'flex', flexDirection: 'column' }}>
        <Stage pendingActivity={thinkingVisible ? undefined : 'Working'} liveItems={thinkingVisible ? items : items.filter(item => item.type !== 'thinking')} running={items[0]?.id !== 'delivery-message'} sessionKey="scroll-fixture" />
      </div>
      <div style={{ height: 300 }} />
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
