import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  ClientEvent,
  ComputerAccessRequestDto,
  ComputerAccessResponseDto,
  PermissionModeId,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';

import {
  appendUserPrompt,
  emptyConversation,
  reduceEvent,
  type ConversationState,
  type UsageSnapshot,
} from './conversation';
import {
  beginTaskRefresh,
  emptyDesktopState,
  reduceDesktopEvent,
  type DesktopState,
} from './desktopState';
import type {
  BootstrapState,
  ConnectionState,
  CredentialMetadata,
  DiagnosticEntry,
  ProviderCredentialMetadata,
  ProviderCredentialUpdate,
  SystemSettingsPane,
  WorkspaceFileSearchResult,
  WorkspaceMetadata,
} from './lingxi';

export interface UseBridge {
  readonly hosted: boolean;
  readonly loading: boolean;
  readonly bootstrap: BootstrapState | null;
  readonly connection: ConnectionState;
  readonly connected: boolean;
  readonly conversation: ConversationState;
  readonly desktop: DesktopState;
  readonly usage: UsageSnapshot | null;
  readonly running: boolean;
  readonly pendingPermission: PermissionRequest | null;
  readonly pendingComputerAccess: ComputerAccessRequestDto | null;
  readonly error: string | null;
  clearError(): void;
  sendPrompt(text: string): Promise<void>;
  runSlashCommand(raw: string): Promise<void>;
  cancel(turnId?: number): Promise<void>;
  approve(requestId: number, response?: PermissionResponseDto): Promise<void>;
  deny(requestId: number): Promise<void>;
  approveComputerAccess(requestId: number, response: ComputerAccessResponseDto): Promise<void>;
  denyComputerAccess(requestId: number): Promise<void>;
  openSystemSettings(pane: SystemSettingsPane): Promise<void>;
  pickWorkspace(): Promise<WorkspaceMetadata | null>;
  selectRecentWorkspace(path: string): Promise<WorkspaceMetadata>;
  searchWorkspaceFiles(query: string): Promise<WorkspaceFileSearchResult>;
  setWorkspaceTrusted(trusted: boolean): Promise<WorkspaceMetadata>;
  setCredential(credential: string): Promise<CredentialMetadata>;
  clearCredential(): Promise<CredentialMetadata>;
  setProviderCredential(providerId: string, credential: string): Promise<ProviderCredentialUpdate>;
  clearProviderCredential(providerId: string): Promise<ProviderCredentialMetadata>;
  setThemePreference(theme: 'dark' | 'light'): Promise<void>;
  restartBridge(): Promise<void>;
  refreshDiagnostics(): Promise<DiagnosticEntry[]>;
  copyDiagnostics(): Promise<void>;
  exportDiagnostics(): Promise<string | null>;
  refresh(): Promise<void>;
  newSession(): Promise<void>;
  resumeSession(sessionId: string): Promise<void>;
  setModel(model: string): Promise<void>;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  refreshTasks(): Promise<void>;
  taskOutput(taskId: string): Promise<void>;
  stopTask(taskId: string): Promise<void>;
}

function getHost() {
  return typeof window !== 'undefined' ? window.lingxi : undefined;
}

function messageFrom(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return 'The desktop host could not complete that action.';
}

export function shouldResetBridgeRuntime(state: ConnectionState): boolean {
  return state.status === 'spawning';
}

export function shouldClearPendingPermissions(state: ConnectionState): boolean {
  return shouldResetBridgeRuntime(state)
    || state.status === 'disconnected'
    || state.status === 'error'
    || state.status === 'idle';
}

export function resetBridgeRuntimeState(): {
  conversation: ConversationState;
  desktop: DesktopState;
  permissionQueue: PermissionRequest[];
  computerAccessQueue: ComputerAccessRequestDto[];
} {
  return {
    conversation: emptyConversation(),
    desktop: emptyDesktopState(),
    permissionQueue: [],
    computerAccessQueue: [],
  };
}

export function useBridge(): UseBridge {
  const hostRef = useRef(getHost());
  const host = hostRef.current;
  const hosted = host !== undefined;
  const [loading, setLoading] = useState(hosted);
  const [bootstrap, setBootstrap] = useState<BootstrapState | null>(null);
  const [connection, setConnection] = useState<ConnectionState>({ status: 'idle' });
  const [conversation, setConversation] = useState<ConversationState>(emptyConversation);
  const [desktop, setDesktop] = useState<DesktopState>(emptyDesktopState);
  const [permissionQueue, setPermissionQueue] = useState<PermissionRequest[]>([]);
  const [computerAccessQueue, setComputerAccessQueue] = useState<ComputerAccessRequestDto[]>([]);
  const [error, setError] = useState<string | null>(null);

  const capture = useCallback((cause: unknown) => {
    const message = messageFrom(cause);
    setError(message);
    throw cause;
  }, []);

  const resetRuntime = useCallback(() => {
    const next = resetBridgeRuntimeState();
    setConversation(next.conversation);
    setDesktop(next.desktop);
    setPermissionQueue(next.permissionQueue);
    setComputerAccessQueue(next.computerAccessQueue);
  }, []);

  const requestTaskList = useCallback(async () => {
    if (!host) return;
    setDesktop((previous) => beginTaskRefresh(previous));
    try {
      await host.command({ type: 'task_list' });
    } catch (cause) {
      capture(cause);
    }
  }, [capture, host]);

  useEffect(() => {
    if (!host) {
      setLoading(false);
      setError('LingXi Desktop must run inside the signed Electron application.');
      return;
    }

    const offEvent = host.onEvent((event: ClientEvent) => {
      setConversation((previous) => reduceEvent(previous, event));
      setDesktop((previous) => reduceDesktopEvent(previous, event));
      if (event.type === 'error') setError(event.message);
    });
    const offState = host.onConnectionStateChanged((state) => {
      setConnection(state);
      if (shouldResetBridgeRuntime(state)) {
        resetRuntime();
        setError(null);
      } else if (shouldClearPendingPermissions(state)) {
        setPermissionQueue([]);
        setComputerAccessQueue([]);
      }
      if (state.status === 'error') setError(state.message);
      if (state.status === 'disconnected' && state.reason) setError(state.reason);
      if (state.status === 'connected') {
        void host.bootstrap()
          .then((snapshot) => setBootstrap(snapshot))
          .catch((cause: unknown) => setError(messageFrom(cause)));
      }
    });
    const offPermission = host.onPermission((request: PermissionRequest) => {
      setPermissionQueue((previous) => [
        ...previous.filter((entry) => entry.request_id !== request.request_id),
        request,
      ]);
    });
    const offComputerAccess = host.onComputerAccess((request: ComputerAccessRequestDto) => {
      setComputerAccessQueue((previous) => [
        ...previous.filter((entry) => entry.request_id !== request.request_id),
        request,
      ]);
    });

    void host.bootstrap()
      .then((snapshot) => {
        setBootstrap(snapshot);
        setConnection(snapshot.connection);
      })
      .catch((cause: unknown) => setError(messageFrom(cause)))
      .finally(() => setLoading(false));

    return () => {
      offEvent();
      offState();
      offPermission();
      offComputerAccess();
    };
  }, [host]);

  useEffect(() => {
    if (!host || connection.status !== 'connected' || !bootstrap?.workspace.trusted) return;
    void Promise.all([
      host.command({ type: 'list_sessions', limit: 100 }),
      host.command({ type: 'list_models' }),
      requestTaskList(),
      host.command({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] }),
    ]).catch((cause: unknown) => setError(messageFrom(cause)));
  }, [bootstrap?.workspace.trusted, host, connection.status, requestTaskList]);

  const patchBootstrap = useCallback((patch: Partial<BootstrapState>) => {
    setBootstrap((previous) => previous ? { ...previous, ...patch } : previous);
  }, []);

  const sendPrompt = useCallback(async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed || !host) return;
    setConversation((previous) => appendUserPrompt(previous, trimmed));
    try {
      await host.sendPrompt(trimmed);
    } catch (cause) {
      setConversation((previous) => reduceEvent(previous, {
        type: 'error',
        kind: { type: 'transport' },
        message: 'Failed to send the prompt to the engine.',
      }));
      capture(cause);
    }
  }, [capture, host]);

  const runSlashCommand = useCallback(async (raw: string) => {
    const command = raw.trim();
    if (!host || !command.startsWith('/')) return;
    setConversation((previous) => appendUserPrompt(previous, command));
    try {
      await host.command({ type: 'run_slash_command', raw: command });
      // Commands such as plugin/skill reloads can mutate the live registry.
      // Refreshing is cheap and also covers bridge implementations that do not
      // yet push the `commands_changed` event on the intercepted slash path.
      await host.command({ type: 'refresh_listings', which: [{ type: 'slash_commands' }] });
    } catch (cause) {
      setConversation((previous) => reduceEvent(previous, {
        type: 'error',
        kind: { type: 'transport' },
        message: 'Failed to run the slash command.',
      }));
      capture(cause);
    }
  }, [capture, host]);

  const cancel = useCallback(async (turnId?: number) => {
    if (!host) return;
    try { await host.cancel(turnId); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const dropPending = useCallback((requestId: number) => {
    setPermissionQueue((previous) => previous.filter((entry) => entry.request_id !== requestId));
  }, []);

  const dropPendingComputerAccess = useCallback((requestId: number) => {
    setComputerAccessQueue((previous) => previous.filter((entry) => entry.request_id !== requestId));
  }, []);

  const approve = useCallback(async (requestId: number, response?: PermissionResponseDto) => {
    if (!host) return;
    try {
      await host.approve(requestId, response);
      dropPending(requestId);
    } catch (cause) { capture(cause); }
  }, [capture, dropPending, host]);

  const deny = useCallback(async (requestId: number) => {
    if (!host) return;
    try {
      await host.deny(requestId);
      dropPending(requestId);
    } catch (cause) { capture(cause); }
  }, [capture, dropPending, host]);

  const approveComputerAccess = useCallback(async (requestId: number, response: ComputerAccessResponseDto) => {
    if (!host) return;
    try {
      await host.approveComputerAccess(requestId, response);
      dropPendingComputerAccess(requestId);
    } catch (cause) { capture(cause); }
  }, [capture, dropPendingComputerAccess, host]);

  const denyComputerAccess = useCallback(async (requestId: number) => {
    if (!host) return;
    try {
      await host.denyComputerAccess(requestId);
      dropPendingComputerAccess(requestId);
    } catch (cause) { capture(cause); }
  }, [capture, dropPendingComputerAccess, host]);

  const openSystemSettings = useCallback(async (pane: SystemSettingsPane) => {
    if (!host) return;
    try { await host.openSystemSettings(pane); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const pickWorkspace = useCallback(async () => {
    if (!host) return null;
    try {
      const workspace = await host.pickWorkspace();
      if (workspace) patchBootstrap({ workspace });
      return workspace;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const selectRecentWorkspace = useCallback(async (path: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const workspace = await host.setWorkspace(path);
      patchBootstrap({ workspace });
      return workspace;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const searchWorkspaceFiles = useCallback(async (query: string) => {
    if (!host) return { files: [], truncated: false };
    try { return await host.searchWorkspaceFiles(query); } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const setWorkspaceTrusted = useCallback(async (trusted: boolean) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const workspace = await host.setWorkspaceTrusted(trusted);
      patchBootstrap({ workspace });
      return workspace;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setCredential = useCallback(async (credential: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const metadata = await host.setCredential(credential);
      patchBootstrap({ credential: metadata });
      return metadata;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const clearCredential = useCallback(async () => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const credential = await host.clearCredential();
      patchBootstrap({ credential });
      return credential;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const setProviderCredential = useCallback(async (providerId: string, credential: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const update = await host.setProviderCredential(providerId, credential);
      patchBootstrap({
        settings: update.settings,
        credential: update.credential.providerId === 'anthropic' ? update.credential : bootstrap?.credential ?? { configured: false, encryptionAvailable: false },
        providerCredentials: (bootstrap?.providerCredentials ?? []).map((entry) => entry.providerId === providerId ? update.credential : entry),
      });
      return update;
    } catch (cause) { return capture(cause); }
  }, [bootstrap?.credential, bootstrap?.providerCredentials, capture, host, patchBootstrap]);

  const clearProviderCredential = useCallback(async (providerId: string) => {
    if (!host) throw new Error('Desktop host unavailable.');
    try {
      const metadata = await host.clearProviderCredential(providerId);
      patchBootstrap({
        credential: metadata.providerId === 'anthropic' ? metadata : bootstrap?.credential ?? { configured: false, encryptionAvailable: false },
        providerCredentials: (bootstrap?.providerCredentials ?? []).map((entry) => entry.providerId === providerId ? metadata : entry),
      });
      return metadata;
    } catch (cause) { return capture(cause); }
  }, [bootstrap?.credential, bootstrap?.providerCredentials, capture, host, patchBootstrap]);

  const setThemePreference = useCallback(async (theme: 'dark' | 'light') => {
    if (!host) return;
    try {
      const settings = await host.updateSettings({ theme });
      patchBootstrap({ settings });
    } catch (cause) { capture(cause); }
  }, [capture, host, patchBootstrap]);

  const restartBridge = useCallback(async () => {
    if (!host) return;
    setError(null);
    try { await host.restartBridge(); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const refreshDiagnostics = useCallback(async () => {
    if (!host) return [];
    try {
      const diagnostics = await host.diagnostics();
      patchBootstrap({ diagnostics });
      return diagnostics;
    } catch (cause) { return capture(cause); }
  }, [capture, host, patchBootstrap]);

  const copyDiagnostics = useCallback(async () => {
    if (!host) return;
    try { await host.copyDiagnostics(); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const exportDiagnostics = useCallback(async () => {
    if (!host) return null;
    try { return await host.exportDiagnostics(); } catch (cause) { return capture(cause); }
  }, [capture, host]);

  const command = useCallback(async (value: Parameters<NonNullable<typeof host>['command']>[0]) => {
    if (!host) return;
    try { await host.command(value); } catch (cause) { capture(cause); }
  }, [capture, host]);

  const refresh = useCallback(async () => {
    await Promise.all([
      command({ type: 'list_sessions', limit: 100 }),
      command({ type: 'list_models' }),
      requestTaskList(),
      command({ type: 'refresh_listings', which: [{ type: 'status' }, { type: 'doctor' }, { type: 'slash_commands' }] }),
      refreshDiagnostics(),
    ]);
  }, [command, refreshDiagnostics, requestTaskList]);

  const newSession = useCallback(
    () => command({ type: 'new_session', model: desktop.currentModel ?? undefined }),
    [command, desktop.currentModel],
  );
  const resumeSession = useCallback(
    (sessionId: string) => command({ type: 'resume_session', session_id: sessionId }),
    [command],
  );
  const setModel = useCallback((model: string) => command({ type: 'set_model', model }), [command]);
  const setPermissionMode = useCallback(
    (mode: PermissionModeId) => command({ type: 'set_permission_mode', mode }),
    [command],
  );
  const refreshTasks = useCallback(() => requestTaskList(), [requestTaskList]);
  const taskOutput = useCallback(
    (taskId: string) => command({ type: 'task_output', task_id: taskId, offset: 0 }),
    [command],
  );
  const stopTask = useCallback((taskId: string) => command({ type: 'task_stop', task_id: taskId }), [command]);

  return {
    hosted,
    loading,
    bootstrap,
    connection,
    connected: connection.status === 'connected',
    conversation,
    desktop,
    usage: conversation.usage,
    running: conversation.running,
    pendingPermission: permissionQueue[0] ?? null,
    pendingComputerAccess: computerAccessQueue[0] ?? null,
    error,
    clearError: () => setError(null),
    sendPrompt,
    runSlashCommand,
    cancel,
    approve,
    deny,
    approveComputerAccess,
    denyComputerAccess,
    openSystemSettings,
    pickWorkspace,
    selectRecentWorkspace,
    searchWorkspaceFiles,
    setWorkspaceTrusted,
    setCredential,
    clearCredential,
    setProviderCredential,
    clearProviderCredential,
    setThemePreference,
    restartBridge,
    refreshDiagnostics,
    copyDiagnostics,
    exportDiagnostics,
    refresh,
    newSession,
    resumeSession,
    setModel,
    setPermissionMode,
    refreshTasks,
    taskOutput,
    stopTask,
  };
}
