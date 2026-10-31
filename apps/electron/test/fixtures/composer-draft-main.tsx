import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';

import { BetaComposer } from '../../src/renderer/components/BetaDesktop';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { emptyRuntimeCenterState } from '../../src/renderer/bridge/runtimeCenterState';
import { emptyConversation } from '../../src/renderer/bridge/conversation';
import type { AudioOperationDto, AudioOperationResultDto } from '@lingxi/bridge-client';
import {
  defaultNativeAudioSnapshot,
  type NativeAudioCommand,
  type NativeAudioOperationResponse,
  type NativeAudioResponse,
} from '../../src/shared/nativeAudio';

// Interaction selectors use English independently of the host machine's locale.
setLanguagePreference('en');

const projectPath = '/tmp/composer-draft-project';
const mentionsFixture = new URLSearchParams(location.search).has('mentions');
const slashHintsFixture = new URLSearchParams(location.search).has('slash-hints');
const suggestionFeedback: Array<{ sessionId: string; uuid: string; action: string }> = [];
const suggestionContexts: Array<Record<string, unknown>> = [];
const openedSettings: string[] = [];
const permissionChanges: string[] = [];
const mentionSearch = async (query: string) => {
  if (query === 'slow') await new Promise((resolve) => setTimeout(resolve, 250));
  return { files: ['src/main.ts', 'My Files/read me.md'].filter((path) => !query || path.toLowerCase().includes(query.toLowerCase())), directories: ['src', 'My Files'].filter((path) => !query || path.toLowerCase().includes(query.replace(/\/$/, '').toLowerCase())), truncated: false };
};
let resolvePendingSend: (() => void) | undefined;
let sendPending = false;
let lastSentPrompt = '';
let cancelCount = 0;
let modelRefreshCount = 0;
const slashCommands: string[] = [];
const audioRequests: NativeAudioCommand[] = [];
const audioOperationTypes: AudioOperationDto['type'][] = [];
let settlePendingAudioListen: ((result: AudioOperationResultDto) => void) | undefined;
let audioCancelCount = 0;
let audioFinishCount = 0;

function supportedAudioSnapshot() {
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.permissions.microphone = 'granted';
  snapshot.permissions.speech = 'authorized';
  snapshot.recognizerAvailable = true;
  snapshot.localeTag = 'en-US';
  snapshot.voices = [{ id: 'system:fixture', label: 'Fixture voice', languageTag: 'en-US', source: 'system', familyId: 'fixture', isDefault: true }];
  snapshot.capabilities!.supported_operations = ['listen', 'speak'];
  return snapshot;
}

function executeAudioOperation(operation: AudioOperationDto): Promise<NativeAudioOperationResponse> {
  audioOperationTypes.push(operation.type);
  if (operation.type === 'listen') {
    return new Promise((resolve) => {
      settlePendingAudioListen = (result) => {
        settlePendingAudioListen = undefined;
        resolve({ snapshot: supportedAudioSnapshot(), result });
      };
    });
  }
  const result: AudioOperationResultDto = operation.type === 'speak'
    ? { type: 'playback_completed' }
    : operation.type === 'synthesize'
      ? { type: 'synthesized', pcm_base64: 'AQI=', sample_rate_hz: 24_000 }
      : { type: 'failed', error: { kind: 'unsupported', message: 'fixture operation is unsupported' } };
  return Promise.resolve({ snapshot: supportedAudioSnapshot(), result });
}

function cancelAudioOperation(): Promise<void> {
  audioCancelCount += 1;
  settlePendingAudioListen?.({ type: 'failed', error: { kind: 'cancelled', message: 'fixture audio was cancelled' } });
  return Promise.resolve();
}

function finishAudioListen(): Promise<void> {
  audioFinishCount += 1;
  settlePendingAudioListen?.({ type: 'transcript', text: 'dictated text', language: 'en-US' });
  return Promise.resolve();
}

/** The catalog the engine's `model_list` normally fills in. */
const ALL_MODELS = [
  'openrouter/openrouter/auto',
  'openrouter/~anthropic/claude-opus-latest',
  'openrouter/openrouter/free',
  'openrouter/inclusionai/ling-3.0-flash-fin:free',
];

function audioResponse(command: NativeAudioCommand): NativeAudioResponse {
  const idleSnapshot = supportedAudioSnapshot();
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
    execute: executeAudioOperation,
    cancel: cancelAudioOperation,
    onEvent: () => () => undefined,
  },
};

function bridgeFixture(sessionId: string | null, running: boolean, backgroundStatus: string | undefined, isCancelling: boolean, models: string[], connected: boolean, sessionLoading: boolean) {
  const activeSession = sessionId ? { projectPath, sessionId } : null;
  return {
    // Whether anything is up to take a command — all the model control is
    // gated on, and independent of the `ready` that gates SENDING.
    hosted: true,
    loading: false,
    connected: connected && !sessionLoading,
    sessionLoading,
    audioSnapshot: supportedAudioSnapshot(),
    audioRealtimeState: { phase: 'idle' },
    audioRealtimeCommand: async () => undefined,
    audioExecute: executeAudioOperation,
    audioCancel: cancelAudioOperation,
    audioFinishListen: finishAudioListen,
    activeSession,
    bootstrap: {
      revision: 1,
      settings: {
        version: 1,
        projects: [projectPath],
        activeProject: projectPath,
        activeSession,
        pinnedSessions: [],
      },
      workspace: { path: projectPath, trusted: true },
      activeSession,
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
      slashCommands: slashHintsFixture ? [
        { name: 'code-review', description: 'Review code', source: 'builtin', argument_hint: '[low|medium|high|xhigh|max|ultra] [--fix] [--comment] [<pr#>|<branch>|<path>] [--max-findings <n>|all]' },
        { name: 'deploy', description: 'Deploy', source: 'project', aliases: ['ship'], argument_hint: '[environment] [version] [region]', argument_names: ['environment', 'version', 'region'] },
        { name: 'raw', description: 'Raw hint', source: 'project', argument_hint: '  <文件>\t[选项]  ' },
      ] : mentionsFixture ? [{ name: 'goal', description: 'Set a goal to keep pursuing', source: 'builtin', aliases: [], hidden: false }, { name: 'plan', description: 'Turn plan mode on', source: 'builtin', aliases: [], hidden: false }] : [],
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
    ...(mentionsFixture ? {
      skillsEvent: { skills: [{ name: 'review', source_dir: '/skills/review' }, { name: '设计规范', source_dir: '/skills/设计规范' }] },
      pluginCatalogEvent: { catalog_json: JSON.stringify({ installed: [{ id: 'browser@local', name: 'Browser', description: 'Control the in-app browser' }] }) },
      skillCatalogEvent: { catalog_json: JSON.stringify({ entries: [{ directory: '/skills/review', description: 'Review code changes' }] }) },
    } : {}),
    refreshSkills: async () => undefined,
    skillAdmin: async () => undefined,
    pluginAdmin: async () => undefined,
    searchWorkspaceFiles: mentionsFixture ? mentionSearch : async () => ({ files: [], truncated: false }),
    previewWorkspaceFile: async (path: string) => ({ kind: 'text', path, size: 20, content: 'export const value = 42;', truncated: false }),
    refreshModelPicker: async () => { modelRefreshCount += 1; },
    refreshSlashCommands: async () => undefined,
    setModel: async () => undefined,
    setReasoningSelection: async () => undefined,
    setFastMode: async () => undefined,
    setPermissionMode: async (mode: string) => { permissionChanges.push(mode); },
    emitCommandOutput: () => undefined,
    beginLocalCommand: async () => undefined,
    runSlashCommand: async (command: string) => { slashCommands.push(command); return true; },
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
  const [sessionId, setSessionId] = useState<string | null>('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
  const [sessionLoading, setSessionLoading] = useState(false);
  const [running, setRunning] = useState(false);
  const [suggestionState, setSuggestionState] = useState({ enabled: true, notice: null as string | null });
  const [suggestion, setSuggestion] = useState<{ text: string; turn: number; sessionId: string }>();
  const [backgroundStatuses, setBackgroundStatuses] = useState<Record<string, string | undefined>>({});
  const [cancellingSessions, setCancellingSessions] = useState<Record<string, boolean>>({});
  const [sidebarWidth, setSidebarWidth] = useState(0);
  const [models, setModels] = useState<string[]>(ALL_MODELS);
  const [ready, setReady] = useState(true);
  const [connected, setConnected] = useState(true);
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [fusionDefault, setFusionDefault] = useState(false);
  const [fusionAdmission, setFusionAdmission] = useState({ user: true, project: true, local: false });

  useEffect(() => {
    window.__composerDraftTest = {
      switchSession: setSessionId,
      setSessionLoading,
      setFusionDefault,
      setFusionScopeAdmitted: (scope, admitted) => setFusionAdmission((current) => ({ ...current, [scope]: admitted })),
      setSidebarWidth,
      setRunning,
      setSuggestion: (text, turn, sessionId) => setSuggestion({ text, turn, sessionId }),
      setSuggestionState: (enabled, notice) => setSuggestionState({ enabled, notice }),
      suggestionFeedback: () => [...suggestionFeedback],
      suggestionContexts: () => [...suggestionContexts],
      setBackgroundStatus: (session, status) => setBackgroundStatuses((current) => ({ ...current, [session]: status })),
      setCancelling: (session, cancelling) => setCancellingSessions((current) => ({ ...current, [session]: cancelling })),
      cancelCount: () => cancelCount,
      slashCommands: () => [...slashCommands],
      sendPending: () => sendPending,
      lastSentPrompt: () => lastSentPrompt,
      resolveSend: () => resolvePendingSend?.(),
      clearAudioRequests: () => { audioRequests.length = 0; audioOperationTypes.length = 0; },
      audioRequestTypes: () => audioRequests.map((request) => request.type),
      audioOperationTypes: () => [...audioOperationTypes],
      audioCancelCount: () => audioCancelCount,
      audioFinishCount: () => audioFinishCount,
      setModels: (next) => setModels(next === 'all' ? ALL_MODELS : next),
      modelRefreshCount: () => modelRefreshCount,
      setReady,
      setConnected,
      setTheme,
      openedSettings: () => [...openedSettings],
      permissionChanges: () => [...permissionChanges],
    };
    return () => { delete window.__composerDraftTest; };
  }, []);

  return (
    <Theme.Provider value={tokens(theme === 'dark')}>
      {/*
        The shell around the composer is what makes composer overlays land or
        get cut: `.desktop-workspace-upper` clips at `overflow: hidden`, and it
        begins where the sidebar ends. Reproduced here so a picker that flies
        out further than that gap fails the way it fails in the app instead of
        merely hanging off an unclipped body.
      */}
      <div style={{ display: 'flex', width: '100vw', height: '100vh', overflow: 'hidden', background: tokens(theme === 'dark').windowBg }}>
        <aside data-testid="fixture-sidebar" style={{ width: sidebarWidth, flexShrink: 0, background: '#101014' }} />
        <div style={{ position: 'relative', flex: 1, minWidth: 0, minHeight: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column', justifyContent: 'flex-end' }}>
          <BetaComposer
            bridge={{ ...bridgeFixture(sessionId, running, sessionId ? backgroundStatuses[sessionId] : undefined, sessionId ? cancellingSessions[sessionId] ?? false : false, models, connected, sessionLoading),
              fusionCatalogEvent: { type: 'fusion_catalog', catalog_json: JSON.stringify({
                scopes: (['user', 'project', 'local'] as const).map((scope) => ({ scope, admitted: fusionAdmission[scope], path: `/fixture/${scope}/settings.json`, revision: 'r1',
                  strategies: [{ id: 'shared', name: '代码审查', config: {} }], default_strategy_id: null, globals: {}, diagnostic: null })),
                effective_globals: {}, diagnostics: [], default_strategy: fusionDefault ? { scope: 'project', id: 'shared' } : null,
              }) },
              fusionAdmin: async () => undefined,
              updatePromptSuggestionContext: async (context: Record<string, unknown>) => { suggestionContexts.push(context); },
              promptSuggestionFeedback: async (sessionId: string, uuid: string, action: string) => { suggestionFeedback.push({ sessionId, uuid, action }); },
              conversation: { ...emptyConversation(), sessionKey: sessionId, suggestionTurn: suggestion?.turn ?? 0,
                promptSuggestionEnabled: suggestionState.enabled, promptSuggestionNotice: suggestionState.notice,
                promptSuggestion: suggestion?.text && suggestion?.sessionId === sessionId ? { type: 'prompt_suggestion', suggestion: suggestion.text, session_id: sessionId, uuid: `suggestion-${suggestion.turn}` } : undefined },
            } as never}
            ready={ready && !sessionLoading}
            onOpenSettings={() => undefined}
            onOpenSettingsPage={(page) => openedSettings.push(page)}
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
      setSuggestion(text: string, turn: number, sessionId: string): void;
      setSuggestionState(enabled: boolean, notice: string | null): void;
      suggestionFeedback(): Array<{ sessionId: string; uuid: string; action: string }>;
      suggestionContexts(): Array<Record<string, unknown>>;
      switchSession(sessionId: string | null): void;
      setSessionLoading(loading: boolean): void;
      setFusionDefault(enabled: boolean): void;
      setFusionScopeAdmitted(scope: 'user' | 'project' | 'local', admitted: boolean): void;
      setTheme(theme: 'dark' | 'light'): void;
      openedSettings(): string[];
      permissionChanges(): string[];
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
      audioOperationTypes(): AudioOperationDto['type'][];
      audioCancelCount(): number;
      audioFinishCount(): number;
      setModels(models: string[] | 'all'): void;
      modelRefreshCount(): number;
      setReady(ready: boolean): void;
      setConnected(connected: boolean): void;
    };
  }
}

createRoot(document.getElementById('root')!).render(<Fixture />);
