import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';
import '../../src/renderer/components/TerminalPanel.css';
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { TerminalPanel, useTerminalPanel } from '../../src/renderer/components/TerminalPanel';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { TerminalEvent, TerminalSnapshot } from '../../src/shared/terminal';
// Interaction selectors use English independently of the host machine's locale.
localStorage.setItem(LANGUAGE_STORAGE_KEY, 'en');

const records = new Map<string, TerminalSnapshot>();
const listeners = new Set<(event: TerminalEvent) => void>();
const acks: unknown[] = []; const inputs: unknown[] = []; const sizes: unknown[] = []; const closed: string[] = [];
let nextId = 0; let failClose = false; let holdNextList = false; let releaseList: (() => void) | undefined;
let createUnknownNext = false; let staleNextList = false;
const emit = (event: TerminalEvent) => listeners.forEach(listener => listener(event));
window.lingxi = { terminal: {
  list: async scope => {
    const snapshot = [...records.values()].filter(tab => tab.scope.sessionId === scope.sessionId).map(tab => ({ ...tab }));
    if (staleNextList) { staleNextList = false; for (const tab of snapshot) { tab.status = 'running'; tab.exitCode = null; delete tab.unknownReason; } }
    if (holdNextList) { holdNextList = false; await new Promise<void>(resolve => { releaseList = resolve; }); }
    return snapshot;
  },
  create: async scope => { const tab: TerminalSnapshot = { id: `terminal-${++nextId}`, scope, title: 'LingXi-Next', status: 'running', exitCode: null, output: '(base) user@MacBook LingXi-Next % ', sequence: 0 }; if (createUnknownNext) { createUnknownNext = false; tab.status = 'unknown'; tab.unknownReason = 'exit-observation-unavailable'; } records.set(tab.id, tab); return { ...tab }; },
  input: async (id, data) => { inputs.push({ id, data }); }, resize: async (id, cols, rows) => { sizes.push({ id, cols, rows }); },
  close: async id => { if (failClose) throw new Error('Close failed'); closed.push(id); records.delete(id); emit({ kind: 'closed', terminalId: id }); },
  acknowledge: async (id, sequence) => { acks.push({ id, sequence }); },
  onEvent: cb => { listeners.add(cb); return () => { listeners.delete(cb); }; },
} } as typeof window.lingxi;
function Fixture() {
  const [session, setSession] = useState('session-a'); const [dark, setDark] = useState(false);
  const controller = useTerminalPanel({ projectPath: '/tmp/LingXi-Next', sessionId: session }, true);
  Object.assign(window, { terminalFixture: { state: () => ({ acks, inputs, sizes, closed, selected: controller.selected?.id, open: controller.open, session, count: records.size, listHeld: Boolean(releaseList) }), session: setSession, theme: setDark, failClose: (value: boolean) => { failClose = value; },
    holdList: () => { holdNextList = true; }, releaseList: () => { const release = releaseList; releaseList = undefined; release?.(); },
    createUnknownNext: () => { createUnknownNext = true; }, staleNextList: () => { staleNextList = true; },
    migrate: (id: string, sessionId: string) => { const tab = records.get(id)!; tab.scope = { ...tab.scope, sessionId }; emit({ kind: 'scope', terminalId: id, scope: tab.scope }); },
    output: (id: string, data: string, reset = false) => { const tab = records.get(id)!; tab.output = reset ? data : tab.output + data; tab.sequence++; emit({ kind: reset ? 'reset' : 'output', terminalId: id, data, sequence: tab.sequence }); },
    duplicate: (id: string) => { const tab = records.get(id)!; emit({ kind: 'output', terminalId: id, data: 'DUPLICATE', sequence: tab.sequence }); },
    exit: (id: string) => { const tab = records.get(id)!; tab.status = 'exited'; tab.exitCode = 7; emit({ kind: 'exit', terminalId: id, exitCode: 7 }); },
    unknown: (id: string) => { const tab = records.get(id)!; tab.status = 'unknown'; tab.unknownReason = 'exit-observation-unavailable'; emit({ kind: 'unknown', terminalId: id, reason: tab.unknownReason }); },
  } });
  return <Theme.Provider value={tokens(dark)}><div style={{ height: '100vh', display: 'flex', color: dark ? '#eee' : '#282a31', background: dark ? '#191a1f' : '#fff', fontFamily: 'system-ui' }}>
    <aside style={{ width: 220, padding: 16, boxSizing: 'border-box', background: dark ? '#22232a' : '#f4f4f6' }}>LingXi Code<br /><br />Projects<br /><br />LingXi-Next</aside>
    <div className="desktop-workspace"><div className="desktop-workspace-upper"><main style={{ flex: 1, padding: 20, display: 'flex', flexDirection: 'column' }}><header>Desktop shell <button id="toggle" onClick={controller.toggle}>Toggle terminal</button></header><div style={{ flex: 1, padding: 48 }}>The terminal runs independently of your conversation.</div><textarea id="composer" placeholder="Ask anything" style={{ height: 70, borderRadius: 18, padding: 14 }} /></main><aside style={{ width: 220, borderLeft: '1px solid #ddd', padding: 18 }}>Subagents<br /><br />No active agents</aside></div><TerminalPanel controller={controller} /></div>
  </div></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
