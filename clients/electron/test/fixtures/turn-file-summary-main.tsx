import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { TurnFileSummary } from '../../src/renderer/components/TurnFileSummary';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const files = ['clients/electron/src/renderer/components/Stage.tsx', 'clients/electron/src/renderer/global.css'].map((path, index) => ({
  path, additions: index ? 2 : 44, removals: index ? 0 : 10,
  diffs: [{ file_path: path, additions: index ? 2 : 44, removals: index ? 0 : 10, gutter_width: 2, truncated_rows: 0,
    rows: [{ kind: 'add' as const, line_no: 1, hunk: 0, segments: [{ text: 'Example change', class: 'plain' as const }] }] }],
}));
function Fixture() {
  const [open, setOpen] = useState(false);
  const t = tokens(new URLSearchParams(location.search).has('dark'));
  return <Theme.Provider value={t}><main style={{ padding: 24, minHeight: '100vh', background: t.transcriptBg }}>
    <TurnFileSummary id="turn-1" files={files} open={open} onSetOpen={(_, next) => setOpen(next)} />
  </main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
