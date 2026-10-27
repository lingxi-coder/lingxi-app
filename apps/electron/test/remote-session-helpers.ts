import type { RemoteFileStore } from '../src/main/remoteControl/fileStore.js';
/**
 * Shared harness of the Remote Control session-layer tests (not a test file
 * itself): a fake session runtime that behaves like the observable parts of
 * `SessionRuntime`, a recording viewer channel, and event builders.
 */

import type {
  AskUserQuestionRequestDto,
  ClientCommand,
  ClientEvent,
  ImageRefDto,
  MediaContentDto,
  MediaConnectionRequestDto,
  MediaConnectionResponseDto,
  MediaControlRequestDto,
  MediaControlResponseDto,
  MediaOperationDto,
  MediaOperationIdDto,
  MediaResultDto,
  PermissionRequest,
} from '@lingxi/bridge-client';
import { ManualClock, RemoteRpcError, type DevicePlatform, type RemotePending } from '@lingxi/bridge-client/remote';

import type {
  ConnectionState,
  HostPermissionRequest,
  InteractionRecord,
  PromptJournalEntry,
  PromptSource,
  RemoteObservation,
  RemoteObserver,
  RemoteRuntimeView,
  SequencedRuntimeEventEnvelope,
  SessionRef,
} from '../src/main/bridgeTypes.js';
import { RemoteCapacityError } from '../src/main/sessionRuntimeManager.js';
import type { RemoteViewer, RemoteViewerChannel } from '../src/main/remoteControl/gateway.js';
import type { RemoteRuntime, RemoteRuntimes, RuntimeLease } from '../src/main/remoteControl/remoteRuntime.js';

export const SESSION_A = '12121212-3434-4567-8899-aaaaaaaaaaaa';
export const SESSION_B = '12121212-3434-4567-8899-bbbbbbbbbbbb';
export const SESSION_C = '12121212-3434-4567-8899-cccccccccccc';
export const SESSION_D = '12121212-3434-4567-8899-dddddddddddd';
export const START = 1_700_000_000_000;
export const PROJECT = '/work/project-one';

export const PNG_BASE64 = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4]).toString('base64');

export function envelope(sessionId: string, sequence: number, event: ClientEvent): SequencedRuntimeEventEnvelope<ClientEvent> {
  return Object.freeze({ sessionId, sequence, event });
}

export const permissionRequest = (requestId: number, kind: PermissionRequest['kind'] = { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{"command":"ls"}', default_allow: false }): HostPermissionRequest => ({
  request_id: requestId,
  kind,
});

export const questionRequest = (requestId: number): AskUserQuestionRequestDto => ({
  request_id: requestId,
  questions: [{ question: 'Which one?', header: 'Pick', options: [{ label: 'A', description: 'first' }, { label: 'B', description: 'second' }], multi_select: false }],
});

export function viewer(overrides: Partial<RemoteViewer> = {}): RemoteViewer {
  return { host_id: 'host-1', device_id: 'device-1', device_name: 'Test phone', platform: 'ios' as DevicePlatform, conn_id: 'conn-1', ...overrides };
}

export interface Notification { method: string; params: any }

export class RecordingChannel implements RemoteViewerChannel {
  readonly sent: Notification[] = [];
  readonly requests: Array<{ method: string; params: unknown }> = [];
  notify(method: string, params: unknown): void { this.sent.push({ method, params }); }
  async request(method: 'media.offer' | 'media.control', params: unknown): Promise<unknown> {
    this.requests.push({ method, params });
    if (method === 'media.offer' && typeof params === 'object' && params !== null) {
      const offer = params as { media_session_ref?: unknown; offer_sdp?: unknown };
      return { v: 1, media_session_ref: offer.media_session_ref, answer_sdp: offer.offer_sdp };
    }
    if (method === 'media.control' && typeof params === 'object' && params !== null) {
      const control = params as { media_session_ref: string; generation: number; target_audio_sequence: number; target_sample_frames: number };
      return {
        v: 1,
        media_session_ref: control.media_session_ref,
        generation: control.generation,
        audio_sequence: control.target_audio_sequence,
        accepted_sample_frames: control.target_sample_frames,
        played_sample_frames: control.target_sample_frames,
      };
    }
    return undefined;
  }
  of(method: string): any[] { return this.sent.filter((item) => item.method === method).map((item) => item.params); }
  /** The `session.event` records, in the order they were sent. */
  records(): Array<{ seq: number; record: any }> { return this.of('session.event').map((params) => ({ seq: params.seq, record: params.record })); }
  methods(): string[] { return this.sent.map((item) => item.method); }
  clear(): void { this.sent.length = 0; }
}

/** A runtime with the observable behaviour of `SessionRuntime`: sequences, a base, a journal, pending sets, observers. */
export class FakeRuntime implements RemoteRuntime {
  readonly projectPath: string;
  engineWorkspacePath?: string;
  connectionState: ConnectionState = { status: 'connected' };
  turnActive = false;
  remoteModel: string | undefined = 'model-one';
  readonly runtimeId: string;
  baseSequence = 0;
  base: SequencedRuntimeEventEnvelope<ClientEvent> | undefined;
  events: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  state: SequencedRuntimeEventEnvelope<ClientEvent>[] = [];
  prompts: PromptJournalEntry[] = [];
  interactions: InteractionRecord[] = [];
  droppedThroughSequence = 0;
  lastSequence = 0;
  readonly permissions = new Map<number, HostPermissionRequest>();
  readonly questions = new Map<number, AskUserQuestionRequestDto>();
  readonly observers = new Set<RemoteObserver>();
  readonly calls: Array<{ method: string; args: unknown[] }> = [];
  readonly mediaConnectionResponses: MediaConnectionResponseDto[] = [];
  readonly mediaControlResponses: MediaControlResponseDto[] = [];
  private readonly mediaConnectionListeners = new Set<(request: MediaConnectionRequestDto) => void>();
  private readonly mediaControlListeners = new Set<(request: MediaControlRequestDto) => void>();
  failSend: Error | undefined;
  disposed = false;

  constructor(readonly sessionId: string = SESSION_A, projectPath: string = PROJECT, runtimeId = 'rt1') {
    this.projectPath = projectPath;
    this.runtimeId = runtimeId;
  }

  get epoch(): string { return `${this.runtimeId}:${this.baseSequence}`; }

  remoteView(): RemoteRuntimeView {
    return {
      sessionId: this.sessionId,
      runtimeId: this.runtimeId,
      epoch: this.epoch,
      baseSequence: this.baseSequence,
      base: this.base,
      events: [...this.events],
      state: [...this.state],
      prompts: [...this.prompts],
      interactions: [...this.interactions],
      droppedThroughSequence: this.droppedThroughSequence,
      lastSequence: this.lastSequence,
      pending: this.remotePending(),
      turn: this.turnActive ? { running: true } : { running: false },
      controls: undefined,
      model: this.remoteModel,
      connection: this.connectionState,
    };
  }

  remotePending(): RemotePending {
    return {
      permissions: [...this.permissions.values()].map((request) => ({ ...request })),
      questions: [...this.questions.values()],
      computer_access: [],
    };
  }

  observeRemote(observer: RemoteObserver): () => void {
    this.observers.add(observer);
    return () => { this.observers.delete(observer); };
  }

  emit(observation: RemoteObservation): void {
    for (const observer of [...this.observers]) observer(observation);
  }

  /** A new base (`session_resumed`), as `/clear` or a restart makes. */
  resume(messages: Array<import('@lingxi/bridge-client').MessageDto> = []): SequencedRuntimeEventEnvelope<ClientEvent> {
    const item = envelope(this.sessionId, ++this.lastSequence, { type: 'session_resumed', session_id: this.sessionId, messages } as ClientEvent);
    this.base = item;
    this.baseSequence = item.sequence;
    this.events = [];
    this.state = [];
    this.prompts = [];
    this.interactions = [];
    this.droppedThroughSequence = 0;
    this.emit({ type: 'event', envelope: item });
    return item;
  }

  /** An engine event: retained like a transcript event and announced. */
  push(event: ClientEvent): SequencedRuntimeEventEnvelope<ClientEvent> {
    const item = envelope(this.sessionId, ++this.lastSequence, event);
    if (event.type === 'model_changed') this.state = [...this.state.filter((e) => e.event.type !== event.type), item];
    else this.events.push(item);
    this.emit({ type: 'event', envelope: item });
    return item;
  }

  journal(text: string, source: PromptSource = { origin: 'desktop' }, images: ImageRefDto[] = [], turnId = 1): PromptJournalEntry {
    const entry: PromptJournalEntry = {
      sequence: ++this.lastSequence,
      turnId,
      text,
      origin: source.origin,
      ...(source.deviceName !== undefined ? { deviceName: source.deviceName } : {}),
      images: images.map((image) => ({ media_type: image.media_type, base64: image.base64 })),
    };
    this.prompts.push(entry);
    this.emit({ type: 'prompt', entry });
    return entry;
  }

  askPermission(request: HostPermissionRequest): void {
    this.permissions.set(request.request_id, request);
    const interaction: InteractionRecord = { sequence: ++this.lastSequence, record: { type: 'permission_request', request } };
    this.interactions.push(interaction);
    this.emit({ type: 'interaction', interaction });
    this.emit({ type: 'pending' });
  }

  askQuestion(request: AskUserQuestionRequestDto): void {
    this.questions.set(request.request_id, request);
    this.emit({ type: 'pending' });
  }

  /** The desktop answers (or the request is withdrawn). */
  settle(requestId: number): void {
    this.permissions.delete(requestId);
    this.questions.delete(requestId);
    this.emit({ type: 'pending' });
  }

  dispose(): void {
    this.disposed = true;
    this.connectionState = { status: 'disconnected' };
    this.emit({ type: 'disposed' });
    this.observers.clear();
  }

  pendingPermission(requestId: number): HostPermissionRequest | undefined { return this.permissions.get(requestId); }
  pendingQuestion(requestId: number): AskUserQuestionRequestDto | undefined { return this.questions.get(requestId); }

  promptImage(sequence: number, index: number): { media_type: string; base64: string } | undefined {
    const image = this.prompts.find((entry) => entry.sequence === sequence)?.images[index];
    return image?.base64 === undefined ? undefined : { media_type: image.media_type, base64: image.base64 };
  }

  sendPrompt(text: unknown, images?: unknown, turnId?: unknown, visualizationContext?: unknown, source?: PromptSource): void {
    this.calls.push({ method: 'sendPrompt', args: [text, images, turnId, source] });
    if (this.failSend) throw this.failSend;
    this.journal(String(text), source, (images ?? []) as ImageRefDto[], Number(turnId));
  }

  cancelTurn(turnId: unknown): void { this.calls.push({ method: 'cancelTurn', args: [turnId] }); }

  approvePermission(requestId: number, response?: unknown): void {
    this.calls.push({ method: 'approvePermission', args: [requestId, response] });
    if (!this.permissions.has(requestId)) throw new Error('permission request is not pending');
    this.settle(requestId);
  }

  denyPermission(requestId: number): void {
    this.calls.push({ method: 'denyPermission', args: [requestId] });
    if (!this.permissions.has(requestId)) throw new Error('permission request is not pending');
    this.settle(requestId);
  }

  answerAskUserQuestion(requestId: number, answers: unknown): void {
    this.calls.push({ method: 'answerAskUserQuestion', args: [requestId, answers] });
    if (!this.questions.has(requestId)) throw new Error('AskUserQuestion request is not pending');
    this.settle(requestId);
  }

  cancelAskUserQuestion(requestId: number): void {
    this.calls.push({ method: 'cancelAskUserQuestion', args: [requestId] });
    if (!this.questions.has(requestId)) throw new Error('AskUserQuestion request is not pending');
    this.settle(requestId);
  }

  async dispatchCommand(command: ClientCommand | unknown, source?: PromptSource): Promise<void> {
    this.calls.push({ method: 'dispatchCommand', args: [command, source] });
    if (this.failSend) throw this.failSend;
  }

  async executeMedia(_operation: MediaOperationDto, _limits?: { timeout_budget_ms?: number; max_payload_bytes?: number }): Promise<{ operation_id: MediaOperationIdDto; result: MediaResultDto }> {
    throw new Error('unused media operation in FakeRuntime');
  }

  async readMedia(_mediaRef: string): Promise<MediaContentDto> {
    throw new Error('unused media read in FakeRuntime');
  }

  async cancelMedia(_operationId: MediaOperationIdDto): Promise<void> {
    throw new Error('unused media cancellation in FakeRuntime');
  }

  async registerMediaConnection(connectionId: string, label: string, modes: readonly string[], sourceKinds: readonly ('microphone' | 'camera' | 'screen')[]): Promise<void> {
    this.calls.push({ method: 'registerMediaConnection', args: [connectionId, label, modes, sourceKinds] });
  }

  async unregisterMediaConnection(connectionId: string): Promise<void> {
    this.calls.push({ method: 'unregisterMediaConnection', args: [connectionId] });
  }

  onMediaConnectionRequest(listener: (request: MediaConnectionRequestDto) => void): () => void {
    this.mediaConnectionListeners.add(listener);
    return () => this.mediaConnectionListeners.delete(listener);
  }

  async respondToMediaConnection(response: MediaConnectionResponseDto): Promise<void> {
    this.mediaConnectionResponses.push(response);
  }

  onMediaControlRequest(listener: (request: MediaControlRequestDto) => void): () => void {
    this.mediaControlListeners.add(listener);
    return () => this.mediaControlListeners.delete(listener);
  }

  async respondToMediaControl(response: MediaControlResponseDto): Promise<void> {
    this.mediaControlResponses.push(response);
  }

  emitMediaConnectionRequest(request: MediaConnectionRequestDto): void {
    for (const listener of [...this.mediaConnectionListeners]) listener(request);
  }

  emitMediaControlRequest(request: MediaControlRequestDto): void {
    for (const listener of [...this.mediaControlListeners]) listener(request);
  }

  async scheduledModelCatalog(): Promise<Extract<ClientEvent, { type: 'model_list' }>> {
    return { type: 'model_list', models: ['model-one', 'model-two'], current: 'model-one' } as Extract<ClientEvent, { type: 'model_list' }>;
  }

  callsOf(method: string): unknown[][] { return this.calls.filter((call) => call.method === method).map((call) => call.args); }
}

/** The slice of the manager the gateway uses, over fake runtimes. */
export class FakeRuntimes implements RemoteRuntimes {
  readonly runtimes = new Map<string, FakeRuntime>();
  readonly listeners = new Set<(runtime: RemoteRuntime) => void>();
  readonly opened: SessionRef[] = [];
  readonly releases: string[] = [];
  /** Sessions that exist on disk and can be opened. */
  readonly known = new Set<string>();
  capacity = Number.POSITIVE_INFINITY;
  failOpen: Error | undefined;
  openDelay: Promise<void> | undefined;
  private remoteOpened = 0;

  add(runtime: FakeRuntime): FakeRuntime {
    this.runtimes.set(runtime.sessionId, runtime);
    for (const listener of [...this.listeners]) listener(runtime);
    return runtime;
  }

  get(sessionId: string): RemoteRuntime | undefined { return this.runtimes.get(sessionId); }

  async openForRemote(ref: SessionRef): Promise<RuntimeLease> {
    this.opened.push(ref);
    await this.openDelay;
    if (this.failOpen) throw this.failOpen;
    let runtime = this.runtimes.get(ref.sessionId);
    if (runtime === undefined) {
      if (this.remoteOpened >= this.capacity) throw new RemoteCapacityError();
      this.remoteOpened += 1;
      runtime = this.add(new FakeRuntime(ref.sessionId, ref.projectPath, `rt-${this.opened.length}`));
      runtime.resume();
    }
    const opened = runtime;
    return { runtime: opened, release: () => { this.releases.push(ref.sessionId); } };
  }

  onRuntimeCreated(listener: (runtime: RemoteRuntime) => void): () => void {
    this.listeners.add(listener);
    for (const runtime of this.runtimes.values()) listener(runtime);
    return () => { this.listeners.delete(listener); };
  }
}

export async function flush(turns = 8): Promise<void> {
  for (let i = 0; i < turns; i += 1) await new Promise<void>((resolve) => setImmediate(resolve));
}

export async function rpcCode(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof RemoteRpcError) return error.code;
    throw error;
  }
  throw new Error('expected the call to fail');
}

export function newClock(): ManualClock { return new ManualClock(START); }

// ─────────────────────────────────────────────────────────────────────────────
// Gateway fixture
// ─────────────────────────────────────────────────────────────────────────────

import { RemoteCommands, type ImageCodec, type RemotePromptEcho } from '../src/main/remoteControl/remoteCommands.js';
import { RemotePolicy, type RemotePolicySettings } from '../src/main/remoteControl/policy.js';
import { SessionGateway, LEASE_GRACE_MS } from '../src/main/remoteControl/sessionGateway.js';

export class FakeSettings implements RemotePolicySettings {
  sessions: SessionRef[] = [];
  archived = new Set<string>();
  atStartup = false;
  allowAlways = false;
  failWrites = false;
  getRemoteSessions(): SessionRef[] { return [...this.sessions]; }
  setRemoteSession(ref: SessionRef, enabled: boolean): void {
    if (this.failWrites) throw new Error('disk full');
    this.sessions = this.sessions.filter((item) => item.sessionId !== ref.sessionId);
    if (enabled) this.sessions.push(ref);
  }
  isSessionArchived(ref: SessionRef): boolean { return this.archived.has(ref.sessionId); }
  getRemoteControlAtStartup(): boolean { return this.atStartup; }
  setRemoteControlAtStartup(enabled: boolean): void { this.atStartup = enabled; }
  getRemoteAllowAlwaysAllow(): boolean { return this.allowAlways; }
}

export interface GatewayFixture {
  gateway: SessionGateway;
  runtimes: FakeRuntimes;
  settings: FakeSettings;
  policy: RemotePolicy;
  clock: ReturnType<typeof newClock>;
  echoes: RemotePromptEcho[];
  titles: Map<string, string>;
  diagnostics: string[];
  channel: RecordingChannel;
  phone: RemoteViewer;
  /** Switch Remote Control on for a session the fake runtimes know. */
  enable(sessionId: string, projectPath?: string): void;
  call(method: string, params?: unknown, who?: { viewer: RemoteViewer; channel: RecordingChannel }): Promise<any>;
}

export function gatewayFixture(options: { codec?: ImageCodec; turnIds?: number[]; files?: RemoteFileStore } = {}): GatewayFixture {
  const clock = newClock();
  const settings = new FakeSettings();
  const policy = new RemotePolicy({ settings });
  const runtimes = new FakeRuntimes();
  const echoes: RemotePromptEcho[] = [];
  const titles = new Map<string, string>();
  const diagnostics: string[] = [];
  let turn = 100;
  const turnIds = options.turnIds;
  const commands = new RemoteCommands({
    allowAlwaysAllow: () => policy.allowAlwaysAllow(),
    echo: (echo) => echoes.push(echo),
    clock,
    ...(options.codec ? { codec: options.codec } : {}),
    randomTurnId: () => (turnIds && turnIds.length > 0 ? turnIds.shift()! : ++turn),
  });
  const gateway = new SessionGateway({
    runtimes,
    policy,
    files: options.files,
    commands,
    titleOf: async (ref) => titles.get(ref.sessionId),
    modelOf: () => 'saved-model',
    clock,
    diagnostics: (_level, message) => diagnostics.push(message),
  });
  const channel = new RecordingChannel();
  const phone = viewer();
  const fixture: GatewayFixture = {
    gateway, runtimes, settings, policy, clock, echoes, titles, diagnostics, channel, phone,
    enable(sessionId, projectPath = PROJECT) { policy.setSessionEnabled({ projectPath, sessionId }, true); },
    call(method, params, who) {
      return gateway.handle(who?.viewer ?? phone, method, params, who?.channel ?? channel);
    },
  };
  return fixture;
}

export { LEASE_GRACE_MS };
