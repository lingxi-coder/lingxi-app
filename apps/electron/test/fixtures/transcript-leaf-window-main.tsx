import { useMemo, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { Stage } from '../../src/renderer/components/Stage';
import { ToolGroup } from '../../src/renderer/components/ToolGroup';
import { TranscriptLeafWindowScope } from '../../src/renderer/components/TranscriptLeafWindow';
import { readingTextAnchor } from '../../src/renderer/components/transcriptReadingAnchor';
import type { RunItem, ToolRunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

let revision = 0;
let nextTool = 1500;
let olderPage = 0;
const sites = new Map<string, number>();
const retired: { instance_id: string; render_revision: number }[] = [];
const renders: { instance: string; revision: number }[] = [];
Object.assign(window, { lingxi: { modUi: {
  async control(_session: string, request: { subtype: string; instance_id: string }) {
    const next = ++revision;
    if (request.subtype === 'ui_render') { sites.set(request.instance_id, next); renders.push({ instance: request.instance_id, revision: next }); }
    return { response: { tree: null }, metadata: { renderRevision: next } };
  },
  async operation(_session: string, request: { type: string; instance_id: string; render_revision: number }) {
    if (request.type === 'draw_unmount') {
      retired.push(request);
      if (sites.get(request.instance_id) === request.render_revision) sites.delete(request.instance_id);
    }
    return {};
  },
  onFrame: () => () => {}, onInvalidate: () => () => {},
} } });
const tool = (index: number): ToolRunItem => ({ type: 'tool', id: `tool-${index}`, tool: 'Read', status: 'done',
  view: { verb: 'read', label: 'Read', title: `file-${index}.ts`, primary: `file-${index}.ts` },
  nativeInput: { file_path: `file-${index}.ts` }, nativeOutput: { lines: [`Result ${index}`] },
  result: { headline: `Result ${index}`, body: `Tool detail ${index}\n${'Long result body. '.repeat(Math.abs(index % 4) * 8)}`, body_lines: 4, body_truncated: false },
  transcriptUuid: `uuid-tool-${index}` });
const initial = (leading = 0): RunItem[] => [
  ...Array.from({ length: leading }, (_, index): RunItem => ({ type: 'narration', role: 'assistant', id: `leading-${index}`, text: `Earlier message ${index}` })),
  ...Array.from({ length: 1500 }, (_, index) => tool(index)),
  ...Array.from({ length: 150 }, (_, index): RunItem => ({ type: 'thinking', id: `thought-${index}`, done: true,
    text: `Thought ${index}\n\n${'Reasoning paragraph with wrapping content. '.repeat(3 + index % 5)}` })),
  { type: 'narration', role: 'assistant', id: 'tail', text: 'The transcript tail.' },
];
const noReading = () => undefined;
const noTarget = () => false;
const noLayout = () => {};
function DirectGroup() {
  const viewport = useRef<HTMLDivElement>(null);
  const following = useRef(false);
  const [open, setOpen] = useState(true);
  const [choices, setChoices] = useState<Record<string, boolean>>({});
  const group = useMemo(() => ({ type: 'tool-group' as const, id: 'direct', tools: Array.from({ length: 1500 }, (_, index) => tool(index)) }), []);
  const scope = useMemo(() => ({ sessionKey: 'direct', viewport, following, readingId: noReading, targetPending: noTarget, onLayout: noLayout }), []);
  return <TranscriptLeafWindowScope.Provider value={scope}>
    <div ref={viewport} className="desktop-stage" tabIndex={0} style={{ height: 640, overflowY: 'auto' }}>
      <ToolGroup group={group} open={open} toolOpen={id => choices[id]} onSetOpen={(id, next) => id === group.id ? setOpen(next) : setChoices(old => ({ ...old, [id]: next }))} modUiSessionId="leaf-native" />
    </div>
  </TranscriptLeafWindowScope.Provider>;
}
function Fixture() {
  const [items, setItems] = useState(() => initial());
  const [session, setSession] = useState('below-threshold');
  const [target, setTarget] = useState<string | null>(null);
  const [located, setLocated] = useState<boolean | null>(null);
  const [direct, setDirect] = useState(false);
  Object.assign(window, { leafFixture: {
    reveal(uuid: string) {
      // A distinct navigation request may repeat the previous UUID.
      flushSync(() => { setLocated(null); setTarget(null); });
      flushSync(() => setTarget(uuid));
    },
    located: () => located,
    sites: () => [...sites.keys()].filter(id => /^tool-(?:use|result):/.test(id)),
    retired: () => retired,
    renders: () => renders,
    textAnchor() {
      const stage = document.querySelector('.desktop-stage')!;
      const top = stage.getBoundingClientRect().top;
      const leaf = [...stage.querySelectorAll('[data-transcript-leaf-id]')].find(node => node.getBoundingClientRect().bottom > top);
      return leaf ? readingTextAnchor(leaf, top) : undefined;
    },
    prepend() {
      const page = ++olderPage;
      flushSync(() => setItems(old => [
        { type: 'narration', role: 'assistant', id: `older-intro-${page}`, text: 'Prepended historical introduction changes the activity identity.' },
        ...Array.from({ length: 70 }, (_, index) => tool(index - 70 * page)), ...old.filter(item => !item.id.startsWith('older-intro-')),
      ]));
    },
    stream() { flushSync(() => setItems(old => {
      const updated = old.map(item => item.id === 'thought-149' && item.type === 'thinking'
        ? { ...item, done: false, streamed: true, text: `${item.text}\n\nNew streamed reasoning with another wrapped paragraph. ${'Delta. '.repeat(35)}` }
        : item.id === 'tool-1499' && item.type === 'tool' ? { ...item, nativeOutput: { lines: ['Updated output'] } } : item);
      return [...updated.slice(0, -1), tool(nextTool++), updated.at(-1)!];
    })); },
    loadLong() { flushSync(() => { setSession('above-threshold'); setTarget(null); setLocated(null); setItems(initial(250)); }); },
    directGroup() { flushSync(() => setDirect(true)); },
  } });
  return <Theme.Provider value={tokens(false)}><div style={{ height: 640, width: 900, maxWidth: '100vw', display: 'flex', flexDirection: 'column' }}>
    {direct ? <DirectGroup /> : <Stage liveItems={items} running sessionKey={session} modUiSessionId="leaf-native"
      messageTarget={target} onMessageLocated={setLocated} />}
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
