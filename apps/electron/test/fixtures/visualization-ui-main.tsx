import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import type { VisualizationGuestEvent, VisualizationMount } from '../../src/shared/visualization';
import { VisualizationCard, VisualizationPlaceholder, VisualizationUnavailable } from '../../src/renderer/components/VisualizationCard';
import { ToolCall } from '../../src/renderer/components/ToolCall';
import type { ToolRunItem } from '../../src/renderer/model/runItem';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// The product renderer and guest preload are real. Only the host's durable
// backend is replaced: this fixture never opens user sessions or credentials.
type Mode = 'ready' | 'loading' | 'unavailable';
let mode: Mode = 'ready';
let mounts = 0;
const listeners = new Set<(event: VisualizationGuestEvent) => void>();
const skillBody = '# Inline visualization\n\nPublish a small, self-contained HTML widget with the Visualization tool.\n\n## When to use it\n\nData that reads better as a chart: trends, comparisons and distributions.\n\n## Detailed instructions\n\n' +
  Array.from({ length: 30 }, (_, index) => `Instruction ${index + 1}: preserve the interaction and its saved state.`).join('\n') +
  '\n\nEND_OF_SKILL_INSTRUCTIONS';
const skill: ToolRunItem = {
  type: 'tool', id: 'skill-visualize', tool: 'Skill', status: 'done',
  view: { verb: 'skill', label: 'Skill', title: 'Skill(visualize)', primary: 'visualize' },
  result: { headline: 'Launching skill: visualize', body: skillBody, body_lines: skillBody.split('\n').length },
};

Object.assign(window, { lingxi: { visualization: {
  async mount(): Promise<VisualizationMount | null> {
    mounts += 1;
    if (mode === 'loading') return new Promise(() => undefined);
    if (mode === 'unavailable') return null;
    return { token: 'a'.repeat(64), generation: mounts, docUrl: 'lingxi-viz://visualization/doc/fixture', title: 'A calmer week' };
  },
  async unmount() {},
  async writeState() { return { saved: true, version: 1 }; },
  onGuestEvent(listener: (event: VisualizationGuestEvent) => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
} } });

function Fixture() {
  const [dark, setDark] = useState(false);
  const [skillOpen, setSkillOpen] = useState<boolean | undefined>(undefined);
  const [cardKey, setCardKey] = useState(0);
  const [startExpanded, setStartExpanded] = useState(false);
  const [poolSize, setPoolSize] = useState(0);
  const palette = tokens(dark);
  Object.assign(window, { visualizationFixture: {
    theme(next: boolean) { flushSync(() => setDark(next)); },
    reset(next: Mode = 'ready', expanded = false) {
      mode = next;
      flushSync(() => { setStartExpanded(expanded); setCardKey(current => current + 1); });
    },
    guestEvent(event: VisualizationGuestEvent) { for (const listener of listeners) listener(event); },
    pool(size: number) { flushSync(() => setPoolSize(size)); },
    state() { return { mode, mounts }; },
  } });
  return <Theme.Provider value={palette}>
    <main style={{ height: '100vh', overflow: 'auto', background: palette.transcriptBg, color: palette.text, padding: '48px 24px' }}>
      <section style={{ maxWidth: 760, margin: '0 auto', display: 'grid', gap: 22, minWidth: 0 }}>
        <h1 style={{ fontSize: 22, fontWeight: 600, letterSpacing: '-.025em' }}>Make room for what matters.</h1>
        <p style={{ color: palette.text2, fontSize: 14 }}>Here is a simple view of your week. Adjust it as your priorities change.</p>
        <ToolCall item={skill} open={skillOpen} onSetOpen={(_id, next) => setSkillOpen(next)} />
        <VisualizationCard key={cardKey} sessionId="fixture-session" reference={{ id: 'weekly', revision: 1 }} startExpanded={startExpanded} />
        <div style={{ display: 'grid', gap: 12 }}>
          <VisualizationPlaceholder />
          <VisualizationUnavailable />
        </div>
        <button id="outside-action" type="button" autoFocus style={{ justifySelf: 'start' }}>Continue conversation</button>
      </section>
      {/* Overlapping cards all intersect the viewport; only the production
          LRU policy decides which real guests remain active. */}
      {Array.from({ length: poolSize }, (_, index) => <div key={index}
        style={{ position: 'fixed', top: 0, left: 0, width: 400, opacity: 0, pointerEvents: 'none' }}>
        <VisualizationCard sessionId="fixture-session" reference={{ id: `pool-${index}`, revision: 1 }} />
      </div>)}
    </main>
  </Theme.Provider>;
}

createRoot(document.getElementById('root')!).render(<Fixture />);
