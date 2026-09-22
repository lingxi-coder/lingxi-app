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
let modelRefreshCount = 0;
const slashCommands: string[] = [];
const audioRequests: NativeAudioCommand[] = [];

/** The catalog the engine's `model_list` normally fills in. */
const ALL_MODELS = [
  'openrouter/openrouter/auto',
  'openrouter/~anthropic/claude-opus-latest',
  'openrouter/openrouter/free',
  'openrouter/inclusionai/ling-3.0-flash-fin:free',
];

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

function bridgeFixture(sessionId: string, running: boolean, backgroundStatus: string | undefined, isCancelling: boolean, models: string[], connected: boolean) {
  return {
    // Whether anything is up to take a command — all the model control is
    // gated on, and independent of the `ready` that gates SENDING.
    hosted: true,
    loading: false,
    connected,
    sessionLoading: false,
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
      models,
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
    refreshModelPicker: async () => { modelRefreshCount += 1; },
    refreshSlashCommands: async () => undefined,
    setModel: async () => undefined,
    setReasoningSelection: async () => undefined,
    setFastMode: async () => undefined,
    setPermissionMode: async () => undefined,
    emitCommandOutput: () => undefined,
    beginLocalCommand: () => undefined,
    runSlashCommand: async (command: string) => { slashCommands.push(command); },
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
  const [sidebarWidth, setSidebarWidth] = useState(0);
  const [models, setModels] = useState<string[]>(ALL_MODELS);
  const [ready, setReady] = useState(true);
  const [connected, setConnected] = useState(true);

  useEffect(() => {
    window.__composerDraftTest = {
      switchSession: setSessionId,
      setSidebarWidth,
      setRunning,
      setBackgroundStatus: (session, status) => setBackgroundStatuses((current) => ({ ...current, [session]: status })),
      setCancelling: (session, cancelling) => setCancellingSessions((current) => ({ ...current, [session]: cancelling })),
      cancelCount: () => cancelCount,
      slashCommands: () => [...slashCommands],
      sendPending: () => sendPending,
      lastSentPrompt: () => lastSentPrompt,
      resolveSend: () => resolvePendingSend?.(),
      clearAudioRequests: () => { audioRequests.length = 0; },
      audioRequestTypes: () => audioRequests.map((request) => request.type),
      setModels: (next) => setModels(next === 'all' ? ALL_MODELS : next),
      modelRefreshCount: () => modelRefreshCount,
      setReady,
      setConnected,
    };
    return () => { delete window.__composerDraftTest; };
  }, []);

  return (
    <Theme.Provider value={tokens('dark')}>
      {/*
        The shell around the composer is what makes composer overlays land or
        get cut: `.desktop-workspace-upper` clips at `overflow: hidden`, and it
        begins where the sidebar ends. Reproduced here so a picker that flies
        out further than that gap fails the way it fails in the app instead of
        merely hanging off an unclipped body.
      */}
      <div style={{ display: 'flex', width: '100vw', height: '100vh', overflow: 'hidden' }}>
        <aside data-testid="fixture-sidebar" style={{ width: sidebarWidth, flexShrink: 0, background: '#101014' }} />
        <div style={{ position: 'relative', flex: 1, minWidth: 0, minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column', justifyContent: 'flex-end' }}>
          <BetaComposer
            bridge={bridgeFixture(sessionId, running, backgroundStatuses[sessionId], cancellingSessions[sessionId] ?? false, models, connected) as never}
            ready={ready}
            onOpenSettings={() => undefined}
            onOpenSettingsPage={() => undefined}
            onSetTheme={() => undefined}
            onOpenProviderSettings={() => undefined}
          />
        </div>
      </div>
    </Theme.Provider>
  );
}

declare global {
  interface Window {
    __composerDraftTest?: {
      switchSession(sessionId: string): void;
      setSidebarWidth(width: number): void;
      setRunning(running: boolean): void;
      setBackgroundStatus(sessionId: string, status: string | undefined): void;
      setCancelling(sessionId: string, cancelling: boolean): void;
      cancelCount(): number;
      slashCommands(): string[];
      sendPending(): boolean;
      lastSentPrompt(): string;
      resolveSend(): void;
      clearAudioRequests(): void;
      audioRequestTypes(): string[];
      setModels(models: string[] | 'all'): void;
      modelRefreshCount(): number;
      setReady(ready: boolean): void;
      setConnected(connected: boolean): void;
    };
  }
}

createRoot(document.getElementById('root')!).render(<Fixture />);
