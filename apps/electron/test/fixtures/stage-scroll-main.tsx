import { useRef, useState } from 'react';
import type { TranscriptHistoryState } from '../../src/shared/sessionHistory';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
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
    // Keep real Native Parent UI wrappers mounted even when no Mod draws a tree.
    modUi: {
      control: async () => ({ response: { tree: null } }),
      operation: async () => ({}),
      onFrame: () => () => {},
      onInvalidate: () => () => {},
    },
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
  const [history, setHistory] = useState<TranscriptHistoryState>({ before: null, loading: false });
  const [sessionKey, setSessionKey] = useState('scroll-fixture');
  const historyRequests = useRef(0);
  const historyToolHead = useRef(false);
  const historyResolve = useRef<(() => void) | null>(null);
  const loadHistory = async () => {
    historyRequests.current++;
    setHistory(previous => ({ ...previous, loading: true }));
    await new Promise<void>(resolve => { historyResolve.current = resolve; });
    flushSync(() => {
      setItems(previous => [...Array.from({ length: 60 }, (_, index): RunItem => index === 59 && historyToolHead.current
        ? { type: 'tool', id: 'older-tool', tool: 'Read', status: 'done', view: { verb: 'read', label: 'Read', title: 'Read older.ts', primary: 'older.ts' } }
        : { type: 'narration', id: `older-${index}`, role: 'assistant', text: `Older saved message ${index}` }), ...previous]);
      setHistory({ before: null, loading: false });
    });
  };
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
    historySetup(count: number, toolHead = false) {
      historyRequests.current = 0;
      historyToolHead.current = toolHead;
      flushSync(() => {
        setSessionKey(`history-fixture-${count}`);
        setItems([
          ...(toolHead ? [{ type: 'tool' as const, id: 'current-tool', transcriptUuid: 'current-tool', tool: 'Read', status: 'done' as const, view: { verb: 'read', label: 'Read', title: 'Read current.ts', primary: 'current.ts' } }] : []),
          ...Array.from({ length: count }, (_, index): RunItem => ({ type: 'narration', id: `history-current-${index}`, transcriptUuid: `history-current-${index}`, role: 'assistant', text: `Current history ${index}: a stable row while earlier messages load.` })),
        ]);
        setHistory({ before: { uuid: '00000000-0000-4000-8000-000000000001' }, loading: false });
      });
    },
    historyRequests: () => historyRequests.current,
    completeHistory: () => historyResolve.current?.(),
    thinking(visible: boolean) { flushSync(() => setThinkingVisible(visible)); },
    activity(phase: 'running' | 'empty-result' | 'result' | 'reasoning' | 'next-tool' | 'group-result' | 'long-reasoning') {
      const read: ToolRunItem = { type: 'tool', id: 'activity-read', tool: 'Read', status: phase === 'running' ? 'running' : 'done',
        view: { verb: 'read', label: 'Read', title: 'Read Stage.tsx', primary: 'Stage.tsx' },
        ...(phase === 'running' || phase === 'empty-result' ? {} : { result: { headline: 'Read 24 lines', body: 'File contents', body_lines: 1, body_truncated: false } }),
      };
      const grouped = ['next-tool', 'group-result', 'long-reasoning'].includes(phase);
      flushSync(() => setItems([
        ...initialItems.slice(0, -1), read,
        ...(phase === 'reasoning' || grouped ? [{ type: 'thinking', id: 'activity-reasoning', streamed: true, done: grouped, text: 'Checking the transcript spacing' } as const] : []),
        ...(grouped ? [{ ...read, id: 'activity-next', status: phase === 'next-tool' ? 'running' : 'done', result: phase === 'next-tool' ? undefined : read.result }] : []),
        ...(phase === 'long-reasoning' ? [{ type: 'thinking', id: 'activity-long', streamed: true, text: '核对 thinking 与工具调用的间距。\n'.repeat(40) } as const] : []),
      ]));
    },
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
          <Stage history={history} onLoadMoreHistory={loadHistory} modUiSessionId="scroll-fixture" pendingActivity={thinkingVisible ? undefined : 'Working'} liveItems={thinkingVisible ? items : items.filter(item => item.type !== 'thinking')} running={items[0]?.id !== 'delivery-message'} sessionKey={sessionKey} foldedItemIds={foldedItems} />
        </div>
        {inspector && <aside className="runtime-inspector" style={{ width: 390 }}>Inspector</aside>}
      </div>
      <div style={{ height: 300 }} />
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
