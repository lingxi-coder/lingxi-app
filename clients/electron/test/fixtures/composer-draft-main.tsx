import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';

import { BetaComposer } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { emptyRuntimeCenterState } from '../../src/renderer/bridge/runtimeCenterState';
import {
  defaultNativeAudioSnapshot,
  type NativeAudioCommand,
  type NativeAudioResponse,
} from '../../src/shared/nativeAudio';

const projectPath = '/tmp/composer-draft-project';
let resolvePendingSend: (() => void) | undefined;
let sendPending = false;
let lastSentPrompt = '';
let cancelCount = 0;
const audioRequests: NativeAudioCommand[] = [];

function audioResponse(command: NativeAudioCommand): NativeAudioResponse {
  const idleSnapshot = defaultNativeAudioSnapshot();
  switch (command.type) {
    case 'request_authorization':
      return { type: 'authorization', snapshot: idleSnapshot };
    case 'start_listening':
      return {
        type: 'listening_started',
        snapshot: { ...idleSnapshot, owner: command.owner, activity: 'listening' },
      };
    case 'finish_listening':
      return {
        type: 'listening_finished',
        snapshot: idleSnapshot,
        transcript: { text: 'dictated text', language: 'en-US' },
      };
    case 'cancel':
      return { type: 'cancelled', snapshot: idleSnapshot };
    case 'speak':
      return { type: 'speaking_started', snapshot: { ...idleSnapshot, owner: command.owner, activity: 'speaking' } };
    case 'stop_speaking':
      return { type: 'speaking_stopped', snapshot: idleSnapshot };
    case 'list_models':
      return { type: 'models', snapshot: idleSnapshot, models: [] };
    case 'install_model':
    case 'cancel_model':
    case 'remove_model':
      return {
        type: 'model_operation',
        snapshot: idleSnapshot,
        model: { modelId: command.modelId, state: { type: 'not-installed' } },
      };
    case 'get_snapshot':
      return { type: 'snapshot', snapshot: idleSnapshot };
  }
}

if (!window.lingxi) (window as unknown as { lingxi: { audio: unknown } }).lingxi = {
  audio: {
    request: async (command: NativeAudioCommand) => {
      audioRequests.push(command);
      return audioResponse(command);
    },
    onEvent: () => () => undefined,
  },
};

function bridgeFixture(sessionId: string, running: boolean, backgroundStatus: string | undefined, isCancelling: boolean) {
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
      providerCredentials: [
        { providerId: 'anthropic', configured: true, encryptionAvailable: true },
        { providerId: 'openrouter', configured: true, encryptionAvailable: true },
      ],
      connection: { status: 'connected' },
      diagnostics: [],
    },
    desktop: {
      sessions: [],
      activeSessionId: sessionId,
      models: [
        'openrouter/openrouter/auto',
        'openrouter/~anthropic/claude-opus-latest',
        'openrouter/openrouter/free',
        'openrouter/inclusionai/ling-3.0-flash-fin:free',
      ],
      modelDetails: [
        { reference: 'openrouter/openrouter/auto', display_name: 'OpenRouter Auto', pricing: { billing_mode: 'per_token' } },
        { reference: 'openrouter/~anthropic/claude-opus-latest', display_name: 'Anthropic: Claude Opus Latest', pricing: { billing_mode: 'per_token' } },
        { reference: 'openrouter/openrouter/free', display_name: 'OpenRouter Free', pricing: { billing_mode: 'free' } },
        { reference: 'openrouter/inclusionai/ling-3.0-flash-fin:free', display_name: 'InclusionAI: Ling 3.0 Flash Fin (free)', pricing: { billing_mode: 'free' } },
      ],
      currentModel: 'openrouter/openrouter/auto',
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
    isCancelling,
    runtimeCenter: {
      ...emptyRuntimeCenterState(),
      agents: backgroundStatus ? {
        reviewer: { agent_id: 'reviewer', name: 'code-review', agent_type: 'general-purpose', status: backgroundStatus },
      } : {},
    },
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
    cancel: async () => { cancelCount += 1; },
  };
}

function Fixture() {
  const [sessionId, setSessionId] = useState('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
  const [running, setRunning] = useState(false);
  const [backgroundStatuses, setBackgroundStatuses] = useState<Record<string, string | undefined>>({});
  const [cancellingSessions, setCancellingSessions] = useState<Record<string, boolean>>({});

  useEffect(() => {
    window.__composerDraftTest = {
      switchSession: setSessionId,
      setRunning,
      setBackgroundStatus: (session, status) => setBackgroundStatuses((current) => ({ ...current, [session]: status })),
      setCancelling: (session, cancelling) => setCancellingSessions((current) => ({ ...current, [session]: cancelling })),
      cancelCount: () => cancelCount,
      sendPending: () => sendPending,
      lastSentPrompt: () => lastSentPrompt,
      resolveSend: () => resolvePendingSend?.(),
      clearAudioRequests: () => { audioRequests.length = 0; },
      audioRequestTypes: () => audioRequests.map((request) => request.type),
    };
    return () => { delete window.__composerDraftTest; };
  }, []);

  return (
    <Theme.Provider value={tokens('dark')}>
      <BetaComposer
        bridge={bridgeFixture(sessionId, running, backgroundStatuses[sessionId], cancellingSessions[sessionId] ?? false) as never}
        ready
        onOpenSettings={() => undefined}
        onOpenSettingsPage={() => undefined}
        onSetTheme={() => undefined}
        onOpenProviderSettings={() => undefined}
      />
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __composerDraftTest?: {
      switchSession(sessionId: string): void;
      setRunning(running: boolean): void;
      setBackgroundStatus(sessionId: string, status: string | undefined): void;
      setCancelling(sessionId: string, cancelling: boolean): void;
      cancelCount(): number;
      sendPending(): boolean;
      lastSentPrompt(): string;
      resolveSend(): void;
      clearAudioRequests(): void;
      audioRequestTypes(): string[];
    };
  }
}

createRoot(document.getElementById('root')!).render(<Fixture />);
