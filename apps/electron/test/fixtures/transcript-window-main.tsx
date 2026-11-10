import { useState } from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const renders: { sessionId: string; instance: string; props: unknown }[] = [];
const unmounts: unknown[] = [];
let revision = 0;
Object.assign(window, { lingxi: { modUi: {
  async control(sessionId: string, request: { subtype: string; instance_id: string; props: unknown }) {
    if (request.subtype === 'ui_render') renders.push({ sessionId, instance: request.instance_id, props: request.props });
    return { response: { tree: null }, metadata: { renderRevision: ++revision } };
  },
  async operation(_sessionId: string, request: unknown) { unmounts.push(request); return {}; },
  onFrame: () => () => {}, onInvalidate: () => () => {},
} } });

const tool = (id: string, status: ToolRunItem['status'] = 'done'): ToolRunItem => ({
  type: 'tool', id, tool: 'Read', status, nativeInput: { file_path: `${id}.ts` }, nativeOutput: { lines: [id] },
  view: { verb: 'read', label: 'Read', title: `${id}.ts` },
  result: { headline: id, body: `Visible result for ${id}`, body_lines: 1, body_truncated: false },
  transcriptUuid: `uuid-${id}`,
});
function history(count: number, session: string): RunItem[] {
  return Array.from({ length: count }, (_, index): RunItem[] => index === 120 || index === count - 2
    ? [tool(`row-${index}-a`), tool(`row-${index}-b`)]
    : [{ type: 'narration', id: `row-${index}`, role: index % 6 === 0 ? 'user' : 'assistant', transcriptUuid: `uuid-${index}`,
      text: index === 400 ? `${session} row ${index}\n\n${'A long historical message retains its disclosure choice. '.repeat(150)}`
        : `${session} row ${index}: preserved historical message ${'with wrapping text. '.repeat(index % 4)}` }]).flat();
}
const initialTarget = new URLSearchParams(location.search).get('initial-target');
const initialSession = initialTarget ? 'initial-target' : 'short';
const initial = history(initialTarget ? 2000 : 12, initialSession);
function Fixture() {
  const [items, setItems] = useState(initial);
  const [session, setSession] = useState(initialSession);
  const [target, setTarget] = useState<string | null>(initialTarget);
  const [located, setLocated] = useState<boolean | null>(null);
  const [running, setRunning] = useState(false);
  const [foldedItems, setFoldedItems] = useState<readonly string[]>([]);
  const [plans, setPlans] = useState<{ id: string; content: string; status: 'approved' }[]>([]);
  Object.assign(window, { transcriptFixture: {
    load(count: number, nextSession = 'long', nextTarget: string | null = null, foldTarget = false) { flushSync(() => {
      setSession(nextSession); setTarget(nextTarget); setLocated(null);
      setItems(history(count, nextSession).map(item => foldTarget && item.id === 'row-121' && item.type === 'narration' ? { ...item, loopWakeupStreak: 2 } : item));
      setFoldedItems(foldTarget ? ['row-120-a', 'row-120-b'] : []); setRunning(false); setPlans([]);
    }); },
    history: () => JSON.stringify(items),
    renders: () => renders,
    unmounts: () => unmounts,
    reveal(uuid: string) { flushSync(() => { setLocated(null); setTarget(uuid); }); },
    located: () => located,
    phase(phase: string) { flushSync(() => setItems(current => [...current.filter(item => item.id !== 'compact'), { type: 'compaction', id: 'compact', status: 'running', phase, startedAt: 1 }])); },
    settleCompact() { flushSync(() => setItems(current => current.map(item => item.type === 'compaction' ? { ...item, status: 'complete', finishedAt: 2 } : item))); },
    updateTool() { flushSync(() => setItems(current => current.map(item => item.id === 'row-10-b' && item.type === 'tool' ? { ...item, status: 'error', nativeOutput: { error: 'updated result' } } : item))); },
    append() { flushSync(() => { setRunning(true); setItems(current => [...current, { type: 'narration', id: `appended-${current.length}`, role: 'assistant', streamed: true, text: 'Streaming tail' }]); }); },
    sendPrompt() { flushSync(() => setItems(current => [...current, { type: 'narration', id: `submitted-${current.length}`, role: 'user', text: 'An explicit new prompt re-engages the tail.' }])); },
    delta() { flushSync(() => setItems(current => current.map((item, index) => index === current.length - 1 && item.type === 'narration' ? { ...item, text: `${item.text}\n\nAnother streamed paragraph with enough content to wrap across the viewport.` } : item))); },
    plan(content: string) { flushSync(() => setPlans([{ id: 'row-10-a', content, status: 'approved' }])); },
  } });
  return <Theme.Provider value={tokens(false)}>
    <div style={{ height: 600, display: 'flex', flexDirection: 'column' }}>
      <Stage sessionKey={session} modUiSessionId={session} liveItems={items} running={running} submittedPlans={plans} foldedItemIds={foldedItems}
        messageTarget={target} onMessageLocated={setLocated} />
      <textarea aria-label="Unsent draft" defaultValue="Keep this unsent draft" style={{ height: 44, flex: 'none' }} />
    </div>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
