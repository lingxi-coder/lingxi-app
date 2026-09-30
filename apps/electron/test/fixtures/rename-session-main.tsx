import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { RenameSessionDialog } from '../../src/renderer/components/RenameSessionDialog';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

function Fixture() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [open, setOpen] = useState(true);
  const [title, setTitle] = useState('Desktop UI优化');
  const [calls, setCalls] = useState(0);
  Object.assign(window, { renameFixture: {
    fail: () => { setBusy(false); setError('Unable to save the chat name. Try again.'); },
    succeed: () => { setBusy(false); setOpen(false); },
  } });
  return <Theme.Provider value={tokens(false)}>
    <output id="calls">{calls}</output><output id="saved-title">{title}</output>
    {open && <RenameSessionDialog title={title} busy={busy} error={error} onClose={() => setOpen(false)}
      onConfirm={next => { setCalls(value => value + 1); setTitle(next); setBusy(true); setError(''); }} />}
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
