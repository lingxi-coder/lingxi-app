/**
 * What the Remote Control modules use of a session runtime. `SessionRuntime`
 * is one; tests supply another. Keeping it to this list is what lets the rest
 * of the directory run without an engine.
 */

import type {
  AskUserQuestionRequestDto,
  ClientCommand,
  ClientEvent,
  MediaContentDto,
  MediaConnectionRequestDto,
  MediaConnectionResponseDto,
  MediaControlRequestDto,
  MediaControlResponseDto,
  MediaOperationDto,
  MediaOperationIdDto,
  MediaResultDto,
  MediaSessionModeDto,
} from '@lingxi/bridge-client';

import type {
  ConnectionState,
  HostPermissionRequest,
  PromptSource,
  RemoteObserver,
  RemoteRuntimeView,
  SessionRef,
} from '../bridgeTypes.js';

export interface RemoteRuntime {
  readonly sessionId: string;
  readonly projectPath: string;
  readonly engineWorkspacePath?: string;
  readonly connectionState: ConnectionState;
  readonly turnActive: boolean;
  readonly remoteModel: string | undefined;
  observeRemote(observer: RemoteObserver): () => void;
  remoteView(): RemoteRuntimeView;
  remotePending(): RemoteRuntimeView['pending'];
  pendingPermission(requestId: number): HostPermissionRequest | undefined;
  pendingQuestion(requestId: number): AskUserQuestionRequestDto | undefined;
  promptImage(sequence: number, index: number): { media_type: string; base64: string } | undefined;
  sendPrompt(text: unknown, images?: unknown, turnId?: unknown, visualizationContext?: unknown, source?: PromptSource): void | Promise<void>;
  cancelTurn(turnId: unknown): void;
  approvePermission(requestId: number, response?: unknown): void;
  denyPermission(requestId: number): void;
  answerAskUserQuestion(requestId: number, answers: unknown): void;
  cancelAskUserQuestion(requestId: number): void;
  dispatchCommand(command: ClientCommand | unknown, source?: PromptSource): Promise<void>;
  executeMedia(operation: MediaOperationDto, limits?: { timeout_budget_ms?: number; max_payload_bytes?: number }): Promise<{ operation_id: MediaOperationIdDto; incarnation: string; result: MediaResultDto }>;
  readMedia(mediaRef: string): Promise<MediaContentDto>;
  cancelMedia(operationId: MediaOperationIdDto, incarnation: string): Promise<void>;
  registerMediaConnection(connectionId: string, label: string, supportedModes: readonly MediaSessionModeDto[], sourceKinds: readonly ('microphone' | 'camera' | 'screen')[]): Promise<void>;
  unregisterMediaConnection(connectionId: string): Promise<void>;
  onMediaConnectionRequest(listener: (request: MediaConnectionRequestDto) => void): () => void;
  respondToMediaConnection(response: MediaConnectionResponseDto): Promise<void>;
  onMediaControlRequest(listener: (request: MediaControlRequestDto) => void): () => void;
  respondToMediaControl(response: MediaControlResponseDto): Promise<void>;
  scheduledModelCatalog(): Promise<Extract<ClientEvent, { type: 'model_list' }>>;
}

/** A runtime kept open for a viewer; releasing it lets the runtime cache trim it again. */
export interface RuntimeLease {
  readonly runtime: RemoteRuntime;
  release(): void;
}

/** The slice of `SessionRuntimeManager` the gateway uses. */
export interface RemoteRuntimes {
  get(sessionId: string): RemoteRuntime | undefined;
  /** Open (resume) the session without showing it on the desktop, and keep it open until released. Rejects with `RemoteCapacityError` beyond the limit. */
  openForRemote(ref: SessionRef): Promise<RuntimeLease>;
  /** Be told about every runtime, the existing ones at once. */
  onRuntimeCreated(listener: (runtime: RemoteRuntime) => void): () => void;
}
