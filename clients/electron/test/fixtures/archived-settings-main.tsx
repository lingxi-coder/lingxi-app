import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import type { UseBridge } from '../../src/renderer/bridge/useBridge';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// Isolated UI state only: never connects to a real project, engine, or preload.
const records = Array.from({ length: 27 }, (_, index) => ({
  projectPath: index % 2 ? '/fixture/project-beta' : '/fixture/project-alpha',
  sessionId: `archive-${index}`,
  title: `Archived conversation ${String(index).padStart(2, '0')}`,
  archivedAt: new Date(Date.UTC(2026, 8, index + 1)).toISOString(),
}));
function Fixture() {
  const [open, setOpen] = useState(true);
  const calls = useRef<string[][]>([]);
  const fail = useRef(true);
  const closes = useRef(0);
  Object.assign(window, { archivedSettingsFixture: { state: () => ({ open, calls: calls.current, closes: closes.current }) } });
  const bridge = {
    connected: false, sessionLoading: false, settingsSnapshotEvent: null,
    bootstrap: { settings: { archivedSessions: records }, workspace: {} },
    openSession: async (projectPath: string, sessionId: string) => {
      calls.current.push([projectPath, sessionId]);
      if (fail.current) { fail.current = false; throw new Error('Fixture restore failed — retry available'); }
    },
  } as unknown as UseBridge;
  return open ? <SettingsScreen bridge={bridge} theme="light" onTheme={() => {}} initialPageId="archived-chats" onClose={() => { closes.current += 1; setOpen(false); }} /> : <p>Settings closed</p>;
}
createRoot(document.getElementById('root')!).render(<Theme.Provider value={tokens(false)}><Fixture /></Theme.Provider>);
