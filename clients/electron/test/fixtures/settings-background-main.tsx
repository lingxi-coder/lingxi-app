import { StrictMode, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { SettingsBackground } from '../../src/renderer/App';
import { AskUserQuestionPrompt } from '../../src/renderer/components/AskUserQuestionPrompt';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const questionRequest = {
  request_id: 1,
  questions: [{
    question: 'Where should the new message land?',
    header: 'Message placement',
    multi_select: false,
    options: [
      { label: 'Follow the bottom', description: 'Keep the transcript pinned to the newest message.' },
      { label: 'Keep my position', description: 'Leave the reader where they are.' },
    ],
  }],
} as never;

function Fixture() {
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    window.__settingsBackgroundTest = {
      setOpen: setSettingsOpen,
    };
    return () => { delete window.__settingsBackgroundTest; };
  }, []);

  return (
    <Theme.Provider value={tokens(false)}>
    <div>
      <SettingsBackground active={settingsOpen}>
        <button type="button" id="background-button">Background action</button>
        <main id="background-main">Background content</main>
        {settingsOpen && (
          <AskUserQuestionPrompt
            request={questionRequest}
            onSubmit={() => undefined}
            onCancel={() => undefined}
          />
        )}
      </SettingsBackground>
      {settingsOpen && (
        <div
          id="settings-view"
          role="dialog"
          aria-modal="true"
          style={{ position: 'fixed', inset: 0, zIndex: 60 }}
        >
          Settings
        </div>
      )}
    </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __settingsBackgroundTest?: { setOpen(value: boolean): void };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
