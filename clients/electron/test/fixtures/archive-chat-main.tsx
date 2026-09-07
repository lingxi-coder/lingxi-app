import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ArchiveChatDialog } from '../../src/renderer/components/ArchiveChatDialog';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// UI-only callbacks: no real chat, cron, preload, or engine is accessed.
function Fixture() {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [confirms, setConfirms] = useState(0);
  const [closes, setCloses] = useState(0);
  Object.assign(window, { archiveChatFixture: {
    configure: ({ loading = false, busy = false, error = '' } = {}) => {
      setLoading(loading); setBusy(busy); setError(error);
    },
    state: () => ({ open, confirms, closes }),
  } });
  return <main style={{ padding: 40 }}>
    <h1>Project chat</h1><button id="opener" onClick={() => setOpen(true)}>Archive chat</button>
    {open && <ArchiveChatDialog title="Project chat" jobs={[{ id: 'fixture-task', cron: '0 9 * * 1', prompt: '# Weekly project summary\n\nSummarize recent project progress.', recurring: true }]}
      loading={loading} busy={busy} error={error}
      onClose={() => { setCloses((value) => value + 1); setOpen(false); }}
      onConfirm={() => { setConfirms((value) => value + 1); setOpen(false); }} />}
  </main>;
}
const palette = tokens(false);
createRoot(document.getElementById('root')!).render(<Theme.Provider value={palette}><Fixture /></Theme.Provider>);
