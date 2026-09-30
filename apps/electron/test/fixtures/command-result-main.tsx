import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { CommandResultPanel } from '../../src/renderer/components/CommandResultPanel';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';
import '../../src/renderer/components/CommandResultPanel.css';
function Fixture() {
  const [open, setOpen] = useState(false);
  const palette = tokens(new URLSearchParams(location.search).get('theme') === 'dark');
  return <Theme.Provider value={palette}><main style={{ padding: 48, minHeight: '100vh', background: palette.stageBg, color: palette.text }}>
    <button id="open" onClick={() => setOpen(true)}>Usage</button>
    {open && <CommandResultPanel item={{ type: 'command', id: 'usage', name: '/usage', isError: false, output: 'Total cost: $3.88\nTotal duration (API): 1h 55m 27s\nTotal duration (wall): 6h 42m 42s\nTotal code changes: 947 lines added, 284 lines removed\ndeepseek-flash: 3.1m input, 308.0k output, 2.7m cache read\nA future metric without a colon\nvery-long-provider-model-name-over-32-characters: 500 input, 20 output' }} onClose={() => setOpen(false)} />}
  </main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
