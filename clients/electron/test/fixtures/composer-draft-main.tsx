import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { BetaComposer } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

const projectPath = '/tmp/composer-draft-project';
let resolvePendingSend: (() => void) | undefined;
let sendPending = false;
let lastSentPrompt = '';

function bridgeFixture(sessionId: string, running: boolean) {
  return {
    activeSession: { projectPath, sessionId },
    bootstrap: {
      revision: 1,
      settings: {
        version: 1,
        projects: [projectPath],
        activeProject: projectPath,
        activeSession: { projectPath, sessionId },
        pinnedSessions: [],
      },
      workspace: { path: projectPath, trusted: true },
      activeSession: { projectPath, sessionId },
      runtimes: [],
      projectCatalogs: { [projectPath]: { sessions: [] } },
      providerCredentials: [{ providerId: 'anthropic', configured: true, encryptionAvailable: true }],
      connection: { status: 'connected' },
      diagnostics: [],
    },
    desktop: {
      sessions: [],
      activeSessionId: sessionId,
      models: [],
      modelDetails: [],
      currentModel: null,
      conversationControls: null,
      fastMode: false,
      permissionMode: 'default',
      slashCommands: [],
      tasks: {},
      taskOutput: {},
      status: null,
      doctor: null,
    },
    running,
    isCancelling: false,
    searchWorkspaceFiles: async () => ({ files: [], truncated: false }),
    setModel: async () => undefined,
    setReasoningSelection: async () => undefined,
    setFastMode: async () => undefined,
    setPermissionMode: async () => undefined,
    emitCommandOutput: () => undefined,
    beginLocalCommand: () => undefined,
    runSlashCommand: async () => undefined,
    sendPrompt: (text: string) => new Promise<void>((resolve) => {
      lastSentPrompt = text;
      sendPending = true;
      resolvePendingSend = () => {
        sendPending = false;
        resolvePendingSend = undefined;
        resolve();
      };
    }),
    cancel: async () => undefined,
  };
}

function Fixture() {
  const [sessionId, setSessionId] = useState('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
  const [running, setRunning] = useState(false);

  useEffect(() => {
    window.__composerDraftTest = {
      switchSession: setSessionId,
      setRunning,
      sendPending: () => sendPending,
      lastSentPrompt: () => lastSentPrompt,
      resolveSend: () => resolvePendingSend?.(),
    };
    return () => { delete window.__composerDraftTest; };
  }, []);

  return (
    <Theme.Provider value={tokens('light')}>
      <BetaComposer
        bridge={bridgeFixture(sessionId, running) as never}
        ready
        onOpenSettings={() => undefined}
        onSetTheme={() => undefined}
      />
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __composerDraftTest?: {
      switchSession(sessionId: string): void;
      setRunning(running: boolean): void;
      sendPending(): boolean;
      lastSentPrompt(): string;
      resolveSend(): void;
    };
  }
}

createRoot(document.getElementById('root')!).render(<Fixture />);
