import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { SettingsBackground } from '../../src/renderer/App';

function Fixture() {
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    window.__settingsBackgroundTest = {
      setOpen: setSettingsOpen,
    };
    return () => { delete window.__settingsBackgroundTest; };
  }, []);

  return (
    <div>
      <SettingsBackground active={settingsOpen}>
        <button type="button" id="background-button">Background action</button>
        <main id="background-main">Background content</main>
      </SettingsBackground>
      {settingsOpen && <div role="dialog" aria-modal="true">Settings</div>}
    </div>
  );
}

declare global {
  interface Window {
    __settingsBackgroundTest?: { setOpen(value: boolean): void };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
