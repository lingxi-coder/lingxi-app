import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/RuntimeCenter.css';

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
  const [inspector, setInspector] = useState(false);
  const [items, setItems] = useState(initialItems);
  const [thinkingVisible, setThinkingVisible] = useState(true);
  // Rows a closed `/loop` fold hides, driven by the fold probe below.
  const [foldedItems, setFoldedItems] = useState<readonly string[]>([]);
  // Drives the transcript's viewport itself, so a probe can resize it the way
  // a window or panel resize would. A width of 0 means "fill the ancestor".
  const [viewport, setViewport] = useState({ width: 0, height: 420 });
  // The ancestor is deliberately SHORTER than the transcript, so the transcript
  // is never what scrolls first. Probes that need the whole scrollport on
  // screen (to click something pinned to it) raise this.
  const [ancestorHeight, setAncestorHeight] = useState(300);
  Object.assign(window, { stageScrollFixture: {
    thinking(visible: boolean) { flushSync(() => setThinkingVisible(visible)); },
    readingWithInspector() { flushSync(() => {
      setInspector(true);
      setViewport({ width: 0, height: 560 });
      setItems([{ type: 'narration', id: 'long-reading', role: 'assistant', streamed: true, text: Array.from({ length: 300 }, (_, i) => `Reading segment ${i}: preserve these words across window resizing.`).join(' ') }, ...initialItems]);
    }); },
    longList() { flushSync(() => setItems(initialItems)); },
    setViewport(width: number, height: number) { flushSync(() => setViewport({ width, height })); },
    setAncestorHeight(height: number) { flushSync(() => setAncestorHeight(height)); },
    // A prompt the reader sent: the one content change that must re-engage the
    // tail even though they had scrolled away.
    sendPrompt() {
      flushSync(() => setItems((current) => [
        ...current,
        { type: 'narration', id: `prompt-${current.length}`, role: 'user', text: 'A prompt the reader just sent.' },
      ]));
    },
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
    // A `/loop` fold hides a contiguous run of rows. The reader's own prompt
    // can be inside that run, so the fold must not be mistaken for a new prompt.
    foldSetup() {
      flushSync(() => {
        setFoldedItems([]);
        setItems([
          ...Array.from({ length: 20 }, (_, index): RunItem => ({
            type: 'narration', id: `filler-${index}`, role: 'assistant',
            text: `Filler ${index}: enough transcript to scroll away from the tail.`,
          })),
          { type: 'narration', id: 'older-prompt', role: 'user', text: 'An older prompt that stays visible.' },
          { type: 'narration', id: 'folded-prompt', role: 'user', text: 'The prompt a closed fold can hide.' },
          { type: 'narration', id: 'loop-wakeup', role: 'assistant', text: 'Loop wakeup', loopWakeupStreak: 2 },
        ]);
      });
    },
    foldNewestPrompt() { flushSync(() => setFoldedItems(['folded-prompt'])); },
    failCopies(fail: boolean) { copyFails = fail; },
  } });
  return <Theme.Provider value={tokens(false)}>
    <div id="scroll-ancestor" style={{ height: ancestorHeight, overflowY: 'auto' }}>
      <div style={{ display: 'flex' }}>
        <div style={{ flex: viewport.width ? 'none' : 1, minWidth: 0, width: viewport.width || undefined, height: viewport.height, display: 'flex', flexDirection: 'column' }}>
          <Stage pendingActivity={thinkingVisible ? undefined : 'Working'} liveItems={thinkingVisible ? items : items.filter(item => item.type !== 'thinking')} running={items[0]?.id !== 'delivery-message'} sessionKey="scroll-fixture" foldedItemIds={foldedItems} />
        </div>
        {inspector && <aside className="runtime-inspector" style={{ width: 390 }}>Inspector</aside>}
      </div>
      <div style={{ height: 300 }} />
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
