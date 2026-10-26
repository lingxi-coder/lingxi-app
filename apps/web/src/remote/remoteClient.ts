import { uploadFile, downloadFile } from './fileTransfer';
import { validateFileOffer, type FileReceipt, type FileProgress } from '@lingxi/bridge-client/remote';
import type {
  AskUserQuestionRequestDto,
  ConversationControlsDto,
  ImageRefDto,
  MessageDto,
  PermissionModeId,
  PermissionResponseDto,
  ReasoningSelectionDto,
} from '@lingxi/bridge-client';
import {
  RemoteRpcError,
  randomId,
  systemClock,
  type Clock,
  type ModelsListResult,
  type RemoteCursor,
  type RemotePending,
  type RemoteSeqRecord,
  type RemoteSessionSummary,
  type RemoteTurnState,
  type SessionSnapshotParams,
} from '@lingxi/bridge-client/remote';
import { emptyTranscript, TranscriptBuilder, type Transcript } from './sessionModel';
import { displayText } from './text';
import type { ViewerConnection } from './viewerConnection';
import { isSupportedImageMediaType, MAX_IMAGE_ATTACHMENTS } from './reuse';

// ─────────────────────────────────────────────────────────────────────────────
// Session state
// ─────────────────────────────────────────────────────────────────────────────

export type SessionStatus =
  /** Attaching, or waiting for the snapshot. */
  | 'loading'
  | 'live'
  /** The connection is down; what is shown is what was last received. */
  | 'offline'
  /** The desktop refused the attach; `error` says why. */
  | 'error'
  /** The session is no longer shared. */
  | 'closed';

/** A prompt this browser sent that its own record has not come back for yet. */
export interface OutgoingPrompt {
  id: string;
  text: string;
  images: readonly ImageRefDto[];
  files?: readonly string[];
  status: 'sending' | 'failed';
  turnId?: number;
  error?: string;
}

export interface SessionState {
  sessionId: string;
  status: SessionStatus;
  error?: { code: string; message: string };
  transcript: Transcript;
  pending: RemotePending;
  turn: RemoteTurnState;
  controls?: ConversationControlsDto;
  outgoing: readonly OutgoingPrompt[];
  /** Whether messages older than the ones loaded can be fetched. */
  hasOlder: boolean;
  loadingOlder: boolean;
  /** Set when an answer lost the race to another device: shown quietly, then cleared by the next change. */
  answeredElsewhere: boolean;
}

const NO_PENDING: RemotePending = { permissions: [], questions: [], computer_access: [] };
const HISTORY_PAGE = 100;
const ATTACH_TIMEOUT_MS = 60_000;
/** After `session.reset` the desktop sends a new snapshot by itself; if none comes, ask for one. */
const RESET_SNAPSHOT_WAIT_MS = 3000;
/** Live events kept while a snapshot is still being assembled. */
const MAX_BUFFERED_EVENTS = 5000;

function initialState(sessionId: string): SessionState {
  return {
    sessionId,
    status: 'loading',
    transcript: emptyTranscript(),
    pending: NO_PENDING,
    turn: { running: false },
    outgoing: [],
    hasOlder: false,
    loadingOlder: false,
    answeredElsewhere: false,
  };
}

/** The permission modes the picker may offer: available ones, never `bypassPermissions`. */
export function selectablePermissionModes(controls: ConversationControlsDto | undefined): PermissionModeId[] {
  return (controls?.permission.options ?? []).filter((option) => option.available && option.mode !== 'bypassPermissions').map((option) => option.mode);
}

/** How a question's answers are sent: the label for a single choice, labels joined by ", " for several. */
export function questionAnswers(
  request: AskUserQuestionRequestDto,
  chosen: ReadonlyMap<string, readonly string[]>,
  other: ReadonlyMap<string, string>,
): Record<string, string> {
  const entries: Array<[string, string]> = [];
  for (const question of request.questions) {
    const text = other.get(question.question)?.trim();
    if (!question.multi_select) {
      const label = text ? text : chosen.get(question.question)?.[0];
      if (label) entries.push([question.question, label]);
      continue;
    }
    const labels = [...(chosen.get(question.question) ?? []), ...(text ? [text] : [])];
    if (labels.length > 0) entries.push([question.question, labels.join(', ')]);
  }
  return Object.fromEntries(entries);
}

type Request = ViewerConnection['request'];

export interface SessionIo {
  request: Request;
  notify(method: string, params?: unknown): void;
  clock: Clock;
}

function errorMessage(error: unknown): { code: string; message: string } {
  if (error instanceof RemoteRpcError) return { code: error.code, message: displayText(error.message, 200) };
  return { code: 'internal', message: error instanceof Error ? displayText(error.message, 200) : 'error' };
}

/**
 * One session as this browser sees it. It attaches (resuming from the last
 * `(epoch, seq)` it holds when it can), assembles the snapshot the desktop
 * sends in pieces, applies live records in order, and offers the commands.
 * Live records that arrive while a snapshot is still being assembled are
 * held back and applied after it, skipping what the snapshot already covers.
 */
export class SessionController {
  private current: SessionState;
  private readonly listeners = new Set<() => void>();
  private notifyScheduled = false;

  private active = false;
  private attachId = 0;
  private historyId = 0;
  private cursor: RemoteCursor | undefined;
  private baseMessages: MessageDto[] = [];
  private firstMessageIndex = 0;
  private records: RemoteSeqRecord[] = [];
  private builder = new TranscriptBuilder();
  private assembly: { epoch: string; first: number; messages: MessageDto[]; records: RemoteSeqRecord[] } | undefined;
  private held: RemoteSeqRecord[] = [];
  private droppedThroughSeq: number | undefined;
  private awaitingAttach = false;
  private resetTimer: unknown;
  private readonly seenTurns = new Set<number>();
  private readonly images = new Map<string, Promise<string>>();

  constructor(
    readonly sessionId: string,
    private readonly io: SessionIo,
    private readonly connected: () => boolean,
  ) {
    this.current = initialState(sessionId);
  }

  getState = (): SessionState => this.current;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private commit(change: Partial<SessionState>): void {
    this.current = { ...this.current, ...change };
    if (this.notifyScheduled) return;
    this.notifyScheduled = true;
    // Records arrive in bursts; one render per burst is enough.
    queueMicrotask(() => {
      this.notifyScheduled = false;
      for (const listener of [...this.listeners]) listener();
    });
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Attaching
  // ───────────────────────────────────────────────────────────────────────────

  /** The page is showing this session: attach now if connected, and again after every reconnect. */
  activate(): void {
    this.active = true;
    if (this.connected()) void this.attach();
  }

  /** The page left the session. */
  deactivate(): void {
    if (!this.active) return;
    this.active = false;
    this.attachId += 1;
    this.invalidateHistory();
    this.io.clock.clearTimeout(this.resetTimer);
    if (this.connected()) void this.io.request('session.detach', { session_id: this.sessionId }).catch(() => undefined);
  }

  /** The connection became ready again. */
  connectionReady(): void {
    if (this.active) void this.attach();
  }

  connectionLost(): void {
    this.attachId += 1;
    this.invalidateHistory();
    this.assembly = undefined;
    this.awaitingAttach = false;
    if (this.current.status === 'loading' || this.current.status === 'live') this.commit({ status: 'offline' });
  }

  private async attach(): Promise<void> {
    const id = ++this.attachId;
    this.invalidateHistory();
    this.io.clock.clearTimeout(this.resetTimer);
    this.assembly = undefined;
    this.held = [];
    this.droppedThroughSeq = undefined;
    this.awaitingAttach = true;
    this.commit({ status: 'loading', error: undefined });
    const resume = this.cursor === undefined ? undefined : { epoch: this.cursor.epoch, after_seq: this.cursor.seq };
    try {
      const result = await this.io.request(
        'session.attach',
        resume === undefined ? { session_id: this.sessionId } : { session_id: this.sessionId, resume },
        { timeoutMs: ATTACH_TIMEOUT_MS },
      );
      if (id !== this.attachId) return;
      this.awaitingAttach = false;
      if (result.mode === 'delta') {
        if (this.cursor === undefined || result.epoch !== this.cursor.epoch) {
          // A delta for a base we do not have cannot be applied: start over without resuming.
          this.cursor = undefined;
          void this.attach();
          return;
        }
        this.commit({ status: 'live', pending: result.pending });
        if (this.flushHeld()) this.sendPresence();
      } else if (this.current.status === 'live' && this.cursor?.epoch === result.epoch) {
        this.sendPresence(); // the snapshot was complete before the reply was handled
      }
    } catch (error) {
      if (id !== this.attachId) return;
      this.awaitingAttach = false;
      const failure = errorMessage(error);
      if (failure.code === 'closed') this.commit({ status: 'offline' });
      else this.commit({ status: 'error', error: failure });
    }
  }

  private foreground = true;

  /** Tell the desktop whether this browser is looking at the session (it uses this to decide about notifications). */
  setForeground(foreground: boolean): void {
    this.foreground = foreground;
    if (this.current.status === 'live') this.sendPresence();
  }

  private sendPresence(): void {
    this.io.notify('session.presence', { session_id: this.sessionId, foreground: this.foreground });
  }

  // ───────────────────────────────────────────────────────────────────────────
  // From the desktop
  // ───────────────────────────────────────────────────────────────────────────

  handleSnapshot(params: SessionSnapshotParams): void {
    switch (params.phase) {
      case 'begin':
        this.invalidateHistory();
        this.assembly = { epoch: params.epoch, first: params.first_message_index, messages: [], records: [] };
        this.commit({ status: 'loading', error: undefined });
        this.io.clock.clearTimeout(this.resetTimer);
        break;
      case 'messages':
        this.assembly?.messages.push(...params.messages);
        break;
      case 'records':
        this.assembly?.records.push(...params.records);
        break;
      case 'end':
        this.finishSnapshot(params);
        break;
    }
  }

  private finishSnapshot(end: Extract<SessionSnapshotParams, { phase: 'end' }>): void {
    const assembly = this.assembly;
    this.assembly = undefined;
    if (assembly === undefined) return;
    this.baseMessages = assembly.messages;
    this.firstMessageIndex = assembly.first;
    this.records = [...assembly.records].sort((a, b) => a.seq - b.seq);
    this.seenTurns.clear();
    for (const { record } of this.records) if (record.type === 'prompt') this.seenTurns.add(record.turn_id);
    this.cursor = { epoch: end.cursor.epoch, seq: end.cursor.seq };
    this.builder = TranscriptBuilder.from(this.baseMessages, this.records);
    this.builder.setRunning(end.turn.running);
    this.commit({
      status: 'live',
      transcript: this.builder.snapshot(),
      pending: end.pending,
      turn: end.turn,
      controls: end.controls ?? this.controlsFromRecords(),
      hasOlder: this.firstMessageIndex > 0,
      loadingOlder: false,
      outgoing: this.current.outgoing.filter((prompt) => prompt.turnId === undefined || !this.seenTurns.has(prompt.turnId)),
    });
    if (this.flushHeld() && !this.awaitingAttach) this.sendPresence();
  }

  private controlsFromRecords(): ConversationControlsDto | undefined {
    let controls: ConversationControlsDto | undefined;
    for (const { record } of this.records) {
      if (record.type === 'event' && record.event.type === 'conversation_controls_changed') controls = record.event.controls;
    }
    return controls;
  }

  handleEvent(item: RemoteSeqRecord): void {
    if (this.assembly !== undefined || this.awaitingAttach || this.current.status === 'loading') {
      if (this.held.length < MAX_BUFFERED_EVENTS) this.held.push(item);
      else this.droppedThroughSeq = Math.max(this.droppedThroughSeq ?? item.seq, item.seq);
      return;
    }
    this.applyLive(item);
  }

  private flushHeld(): boolean {
    const held = this.held.sort((a, b) => a.seq - b.seq);
    this.held = [];
    const dropped = this.droppedThroughSeq;
    this.droppedThroughSeq = undefined;
    if (dropped !== undefined && (this.cursor === undefined || dropped > this.cursor.seq)) {
      // A full buffer is recoverable, but advancing past the omitted records
      // would make them impossible to recover with a delta. Request a new base.
      this.cursor = undefined;
      this.commit({ status: 'loading' });
      if (this.active && this.connected()) void this.attach();
      return false;
    }
    for (const item of held) this.applyLive(item);
    return true;
  }

  private applyLive(item: RemoteSeqRecord): void {
    if (this.cursor === undefined || item.seq <= this.cursor.seq) return; // already covered
    this.cursor = { epoch: this.cursor.epoch, seq: item.seq };
    this.records.push(item);
    this.builder.apply(item.record);
    const change: Partial<SessionState> = { transcript: this.builder.snapshot() };
    const record = item.record;
    if (record.type === 'prompt') {
      this.seenTurns.add(record.turn_id);
      const outgoing = this.current.outgoing.filter((prompt) => prompt.turnId !== record.turn_id);
      if (outgoing.length !== this.current.outgoing.length) change.outgoing = outgoing;
    } else if (record.type === 'event') {
      const event = record.event;
      if (event.type === 'turn_started') change.turn = { running: true, ...(event.turn_id === undefined ? {} : { turn_id: event.turn_id }) };
      else if (event.type === 'turn_ended') change.turn = { running: false };
      else if (event.type === 'conversation_controls_changed') change.controls = event.controls;
      else if (event.type === 'model_changed' && this.current.controls) {
        change.controls = { ...this.current.controls, qualified_model: event.model };
      } else if (event.type === 'permission_mode_changed' && this.current.controls) {
        const permission = this.current.controls.permission;
        change.controls = { ...this.current.controls, permission: { ...permission, requested: event.mode, effective: event.mode } };
      }
    }
    this.commit(change);
  }

  handlePending(pending: RemotePending): void {
    this.commit({ pending, answeredElsewhere: false });
  }

  handleReset(): void {
    this.invalidateHistory();
    this.cursor = undefined;
    this.baseMessages = [];
    this.records = [];
    this.held = [];
    this.droppedThroughSeq = undefined;
    this.assembly = undefined;
    this.firstMessageIndex = 0;
    this.seenTurns.clear();
    this.builder = new TranscriptBuilder();
    this.commit({ status: 'loading', transcript: this.builder.snapshot(), pending: NO_PENDING, turn: { running: false }, hasOlder: false });
    this.io.clock.clearTimeout(this.resetTimer);
    // The desktop sends a fresh snapshot by itself; if it does not, ask.
    this.resetTimer = this.io.clock.setTimeout(() => {
      if (this.active && this.assembly === undefined && this.current.status === 'loading') void this.attach();
    }, RESET_SNAPSHOT_WAIT_MS);
  }

  handleClosed(): void {
    this.attachId += 1;
    this.invalidateHistory();
    this.commit({ status: 'closed' });
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Commands
  // ───────────────────────────────────────────────────────────────────────────

  /** Send a prompt. While a turn runs the desktop takes plain text only, so images are left out then. */
  async sendPrompt(text: string, images: readonly ImageRefDto[] = [], files: readonly string[] = []): Promise<void> {
    const running = this.current.turn.running || this.current.transcript.conversation.running;
    const attachments = running ? [] : images.filter((image) => isSupportedImageMediaType(image.media_type)).slice(0, MAX_IMAGE_ATTACHMENTS);
    if (text.trim().length === 0 && attachments.length === 0 && files.length === 0) return;
    const prompt: OutgoingPrompt = { id: randomId(), text, images: attachments, files, status: 'sending' };
    this.commit({ outgoing: [...this.current.outgoing, prompt] });
    await this.dispatchPrompt(prompt);
  }

  /** Send a failed prompt again with the same request id, so the desktop can tell it is a repeat. */
  async retryPrompt(id: string): Promise<void> {
    const prompt = this.current.outgoing.find((entry) => entry.id === id);
    if (prompt === undefined || prompt.status !== 'failed') return;
    this.updateOutgoing(id, { status: 'sending', error: undefined });
    await this.dispatchPrompt(prompt);
  }

  dismissPrompt(id: string): void {
    this.commit({ outgoing: this.current.outgoing.filter((entry) => entry.id !== id) });
  }

  private async dispatchPrompt(prompt: OutgoingPrompt): Promise<void> {
    try {
      const result = await this.io.request('prompt.send', {
        session_id: this.sessionId,
        text: prompt.text,
        ...(prompt.images.length > 0 ? { images: [...prompt.images] } : {}),
        ...((prompt.files?.length ?? 0) > 0 ? { file_refs: [...prompt.files!] } : {}),
        client_request_id: prompt.id,
      });
      if (this.seenTurns.has(result.turn_id)) this.dismissPrompt(prompt.id);
      else this.updateOutgoing(prompt.id, { turnId: result.turn_id });
    } catch (error) {
      this.updateOutgoing(prompt.id, { status: 'failed', error: errorMessage(error).code });
    }
  }

  private updateOutgoing(id: string, change: Partial<OutgoingPrompt>): void {
    this.commit({ outgoing: this.current.outgoing.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)) });
  }

  async cancelTurn(): Promise<void> {
    const turnId = this.current.turn.turn_id;
    await this.command('turn.cancel', { session_id: this.sessionId, ...(turnId === undefined ? {} : { turn_id: turnId }) });
  }

  /** Allow once or deny; always-allow is not offered from here. */
  async respondPermission(requestId: number, decision: Extract<PermissionResponseDto, { type: 'allow_once' | 'deny' }>): Promise<void> {
    await this.answer(() => this.io.request('permission.respond', { session_id: this.sessionId, request_id: requestId, decision }));
  }

  async answerQuestion(requestId: number, answers: Record<string, string>): Promise<void> {
    await this.answer(() => this.io.request('question.answer', { session_id: this.sessionId, request_id: requestId, answers }));
  }

  async cancelQuestion(requestId: number): Promise<void> {
    await this.answer(() => this.io.request('question.cancel', { session_id: this.sessionId, request_id: requestId }));
  }

  /** First answer wins: losing the race to another device is not an error, only a quiet note. */
  private async answer(send: () => Promise<unknown>): Promise<void> {
    try {
      await send();
    } catch (error) {
      if (error instanceof RemoteRpcError && error.code === 'not_pending') this.commit({ answeredElsewhere: true });
      else throw error;
    }
  }

  async setModel(model: string): Promise<void> {
    await this.command('controls.set_model', { session_id: this.sessionId, model });
  }

  async setReasoning(selection: ReasoningSelectionDto): Promise<void> {
    await this.command('controls.set_reasoning', { session_id: this.sessionId, selection });
  }

  async setPermissionMode(mode: PermissionModeId): Promise<void> {
    if (mode === 'bypassPermissions') return; // never offered, never sent
    await this.command('controls.set_permission_mode', { session_id: this.sessionId, mode });
  }

  private async command<K extends 'turn.cancel' | 'controls.set_model' | 'controls.set_reasoning' | 'controls.set_permission_mode'>(
    method: K,
    params: Parameters<Request>[1],
  ): Promise<void> {
    await this.io.request(method, params as never);
  }

  /** Fetch the page of messages before the ones held. */
  async loadOlder(): Promise<void> {
    if (this.current.status !== 'live' || !this.connected() || !this.current.hasOlder || this.current.loadingOlder) return;
    const id = ++this.historyId;
    this.commit({ loadingOlder: true });
    try {
      const result = await this.io.request('session.history', {
        session_id: this.sessionId,
        before_index: this.firstMessageIndex,
        limit: HISTORY_PAGE,
      });
      if (id !== this.historyId) return;
      this.baseMessages = [...result.messages, ...this.baseMessages];
      this.firstMessageIndex = result.first_index;
      const running = this.current.transcript.conversation.running;
      this.builder = TranscriptBuilder.from(this.baseMessages, this.records);
      this.builder.setRunning(running);
      this.commit({
        transcript: this.builder.snapshot(),
        hasOlder: result.has_more !== false && this.firstMessageIndex > 0,
        loadingOlder: false,
      });
    } catch {
      if (id === this.historyId) this.commit({ loadingOlder: false });
    }
  }

  /** A history page belongs to one transcript base, never to a later reset or attach. */
  private invalidateHistory(): void {
    this.historyId += 1;
    if (this.current.loadingOlder) this.commit({ loadingOlder: false });
  }

  /** The bytes of an image the desktop keeps, as a `data:` URL; fetched once. */
  fetchImage(ref: string): Promise<string> {
    let image = this.images.get(ref);
    if (image === undefined) {
      image = this.io.request('image.get', { ref }).then((result) => {
        if (!isSupportedImageMediaType(result.media_type) || !/^[A-Za-z0-9+/]+={0,2}$/u.test(result.base64)) throw new Error('unsupported image');
        return `data:${result.media_type};base64,${result.base64}`;
      });
      image.catch(() => this.images.delete(ref));
      this.images.set(ref, image);
    }
    return image;
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// RemoteClient
// ─────────────────────────────────────────────────────────────────────────────

export interface RemoteClientState {
  /** `undefined` until the first list arrives. */
  sessions: readonly RemoteSessionSummary[] | undefined;
}

function cleanSummary(summary: RemoteSessionSummary): RemoteSessionSummary {
  const clean: RemoteSessionSummary = {
    ...summary,
    title: displayText(summary.title, 120),
    project_label: displayText(summary.project_label, 60),
    viewers: typeof summary.viewers === 'number' ? summary.viewers : 0,
  };
  if (summary.model !== undefined) clean.model = displayText(summary.model, 80);
  return clean;
}

/**
 * Everything above one connection that the pages share: the list of shared
 * sessions, the per-session controllers (kept for the life of the tab so a
 * session's `(epoch, seq)` survives going back to the list and a reconnect),
 * and the routing of host notifications to them.
 */
export class RemoteClient {
  private current: RemoteClientState = { sessions: undefined };
  private readonly listeners = new Set<() => void>();
  private readonly controllers = new Map<string, SessionController>();
  private readonly clock: Clock;
  private modelsCache: Promise<ModelsListResult> | undefined;
  private readonly offs: Array<() => void> = [];

  constructor(
    private readonly connection: ViewerConnection,
    options: { clock?: Clock } = {},
  ) {
    this.clock = options.clock ?? systemClock;
    this.offs.push(
      connection.onNotification((method, params) => this.route(method, params)),
      connection.onReady(() => this.ready()),
      connection.subscribe((state) => {
        if (state.phase !== 'ready') for (const controller of this.controllers.values()) controller.connectionLost();
      }),
    );
  }

  getState = (): RemoteClientState => this.current;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private commit(change: Partial<RemoteClientState>): void {
    this.current = { ...this.current, ...change };
    for (const listener of [...this.listeners]) listener();
  }

  /** The controller for a session, created on first use and kept. */
  session(sessionId: string): SessionController {
    let controller = this.controllers.get(sessionId);
    if (controller === undefined) {
      controller = new SessionController(
        sessionId,
        {
          request: (method, params, options) => this.connection.request(method, params, options),
          notify: (method, params) => this.connection.notify(method, params),
          clock: this.clock,
        },
        () => this.connection.state.phase === 'ready',
      );
      this.controllers.set(sessionId, controller);
    }
    return controller;
  }

  listFiles(session_id: string): Promise<FileReceipt[]> { return this.connection.request('file.list', { session_id }).then(result => result.files.map(file => {
    const offer = validateFileOffer(file);
    if (offer.session_id !== session_id || file.ref !== offer.transfer_id) throw new Error('Invalid file receipt');
    return { ...offer, ref: file.ref };
  })); }
  uploadFile(session: string, file: File, progress?: FileProgress, signal?: AbortSignal): Promise<FileReceipt> {
    return uploadFile(this.connection, session, file, progress, signal);
  }
  downloadFile(file: FileReceipt, progress?: FileProgress, signal?: AbortSignal): Promise<File> { return downloadFile(this.connection, file, progress, signal); }

  /** The models the desktop offers; asked once per connection. */
  models(): Promise<ModelsListResult> {
    this.modelsCache ??= this.connection.request('models.list', {}).catch((error: unknown) => {
      this.modelsCache = undefined;
      throw error;
    });
    return this.modelsCache;
  }

  private ready(): void {
    this.modelsCache = undefined;
    this.connection
      .request('sessions.watch', {})
      .then((result) => this.commit({ sessions: result.sessions.map(cleanSummary) }))
      .catch(() => undefined);
    for (const controller of this.controllers.values()) controller.connectionReady();
  }

  private route(method: string, params: unknown): void {
    const body = params as Record<string, unknown> | null;
    if (typeof body !== 'object' || body === null) return;
    if (method === 'sessions.changed') {
      const sessions = body['sessions'];
      if (Array.isArray(sessions)) this.commit({ sessions: (sessions as RemoteSessionSummary[]).map(cleanSummary) });
      return;
    }
    const sessionId = body['session_id'];
    const controller = typeof sessionId === 'string' ? this.controllers.get(sessionId) : undefined;
    if (controller === undefined) return;
    switch (method) {
      case 'session.snapshot':
        controller.handleSnapshot(body as unknown as SessionSnapshotParams);
        break;
      case 'session.event':
        controller.handleEvent({ seq: body['seq'] as number, record: body['record'] as RemoteSeqRecord['record'] });
        break;
      case 'session.pending':
        controller.handlePending(body['pending'] as RemotePending);
        break;
      case 'session.reset':
        controller.handleReset();
        break;
      case 'session.closed':
        controller.handleClosed();
        break;
    }
  }

  dispose(): void {
    for (const off of this.offs.splice(0)) off();
    for (const controller of this.controllers.values()) controller.deactivate();
  }
}
