import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import type { PermissionRequest } from '@lingxi/bridge-client';
import { BetaSidebar } from '../../src/renderer/components/BetaDesktop';
import { PermissionPrompt } from '../../src/renderer/components/PermissionPrompt';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

const projectPath = '/tmp/prompt-focus-project';
const request: PermissionRequest = {
  request_id: 1,
  kind: {
    type: 'tool_use_confirm',
    tool_name: 'Read',
    tool_input_json: JSON.stringify({ path: '/tmp/example.txt' }),
    default_allow: false,
  },
};

// This is intentionally the production sidebar component, with only its
// bridge contract stubbed. Keeping the control in BetaSidebar makes this an
// interaction test of the real navigation DOM rather than a surrogate button.
const bridge = {
  bootstrap: {
    revision: 1,
    settings: {
      version: 1,
      projects: [projectPath],
      activeProject: projectPath,
      pinnedSessions: [],
    },
    workspace: { path: projectPath, trusted: true },
    runtimes: [],
    projectCatalogs: { [projectPath]: { sessions: [] } },
    connection: { status: 'connected' },
    diagnostics: [],
  },
  connection: { status: 'connected' },
  sessionRuntimeStatus: () => undefined,
  listProjectSessions: async () => ({ projectPath, sessions: [] }),
  newSession: async () => undefined,
  addProject: async () => undefined,
  activateProject: async () => undefined,
  removeProject: async () => undefined,
  setSessionPinned: async () => undefined,
  openSession: async () => undefined,
};

function Fixture() {
  const [promptOpen, setPromptOpen] = useState(false);

  useEffect(() => {
    // Focus the real sidebar control before opening the prompt. PermissionPrompt
    // should remember it, but must not restore focus after the user has moved
    // back to the global navigation while the prompt is still mounted.
    const timer = window.setTimeout(() => {
      const sidebarControl = document.querySelector<HTMLButtonElement>('button[aria-label="Add project"]');
      sidebarControl?.focus();
      setPromptOpen(true);
    }, 0);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    window.__promptFocusTest = {
      dismissPrompt: () => setPromptOpen(false),
    };
    return () => { delete window.__promptFocusTest; };
  }, []);

  return (
    <Theme.Provider value={tokens('light')}>
      <div style={{ position: 'relative', height: '100vh', display: 'flex' }}>
        {promptOpen && (
          <PermissionPrompt request={request} onApprove={() => {}} onDeny={() => {}} />
        )}
        <BetaSidebar bridge={bridge as never} onOpenSettings={() => {}} />
      </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __promptFocusTest?: { dismissPrompt(): void };
  }
}

createRoot(document.getElementById('root')!).render(<Fixture />);
