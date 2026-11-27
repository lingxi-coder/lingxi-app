import { validateHistoryPage } from './historyPage.js';
import { EventEmitter } from 'node:events';
import type { FirebaseOptions } from 'firebase/app';
import type { Firestore } from 'firebase/firestore';
import { TeamsApiError, teamsTransport, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../../packages/teams';
import type { MessageSignal, MessageSnapshot, MessageCommand, MessageScope, TeamMessage } from '../../../../../packages/teams/messages';
import type { AcceptSharedBotInput, LocalPublishableBot, OfferSharedBotInput, PublishLocalBotInput, PublishedKnowledge, SharedHostStatus, SelectedSharedFile, TriageLogPage, TriageLogRequest, TriageTarget } from '../../../../../packages/teams/execution';
import type { ChatReadRequest, ChatReadResult } from '../../../../../packages/teams/chat-source';
import { createServiceChatReader, isServiceId, isServiceMessageCommand, servicesTransport, type ServiceCommand } from '../../../../../packages/teams/hosted-service';
import { readPrivateDataPage, validPreviewLease, type PrivatePreviewLease, type PrivatePreviewPage, type PrivatePreviewProof } from './dataPreview.js';
import type { DataRequest } from '../../../../../packages/teams/lingxi-data.js';
import type { TeamsFirestoreSession, TeamsFirestoreOptions } from './firestore.js';
import type { TeamDataProof, TeamDatasetOffer } from './process.js';
import type { DatasetBotScope } from '../../../../../packages/teams/dataset-run.js';
import type { DatasetTaskProgram } from './datasetTasks.js';
import { parseVisualizationArtifact } from '../../../../../packages/teams/visualization.js';
import { uploadVisualizationParts, type VisualizationUpload } from '../../../../../packages/teams/visualization-transfer.js';
import type { OpenBotVisualization } from './privateStore.js';

export interface Account {
  snapshot(): { user: { uid: string } | null };
  sessionEpoch?(): number;
  getIdToken(forceRefresh?: boolean): Promise<string>;
  onChanged(callback: () => void): () => void;
}
export interface TeamAccountContext { uid: string; epoch: number }
export interface LocalTeamHost {
  localBots(uid: string): Promise<LocalPublishableBot[]>;
  selectSharedFiles(uid: string, instanceId: string): Promise<SelectedSharedFile[]>;
  publishedKnowledge(uid: string, teamId: string, sharedBotId: string): Promise<PublishedKnowledge>;
  triageTargets(uid: string, instanceId: string): Promise<TriageTarget[]>;
  triageLog(uid: string, request: TriageLogRequest): Promise<TriageLogPage>;
  datasetBotProgram?(uid: string, scope: DatasetBotScope): Promise<DatasetTaskProgram>;
  publishLocalBot(uid: string, input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }>;
  offerSharedBot(uid: string, input: OfferSharedBotInput): Promise<void>;
  acceptSharedBot(uid: string, input: AcceptSharedBotInput): Promise<{ sharedBotId: string; revision: number }>;
  status(uid: string): Promise<SharedHostStatus>;
  revoke(): void;
  stopBinding(teamId: string, sharedBotId: string): void;
  close(): void;
}
/** Main owns the credential and SDK; IPC carries only typed membership data. */
export class TeamsService {
  private readonly events = new EventEmitter();
  private uid?: string;
  private epoch = 0;
  private authenticationEpoch: number;
  private session?: TeamsFirestoreSession;
  private opening?: Promise<TeamsFirestoreSession>;
  private dataSession?: import('./messageWindow.js').TeamsMessageSession;
  /** Sessions of hosted Bot services this account's host follows; a new account starts with none. */
  private serviceChat?: { epoch: number; reader: ReturnType<typeof createServiceChatReader> };
  private dataOperation: Promise<void> = Promise.resolve();
  private dataOpening?: Promise<import('./messageWindow.js').TeamsMessageSession>;
  private sdk?: typeof import('./firestore.js');
  private readonly messageLeases = new Map<string, { uid: string; teamId: string; generation: number; conversationId: string }>();
  private stops = new Map<string, () => void>();
  private failed = new Map<string, number>();
  private readonly stopAccount: () => void;
  private host?: LocalTeamHost;
  constructor(private readonly account: Account, private readonly config: FirebaseOptions, private readonly base = 'https://lingxios.net',
    private readonly emulator?: TeamsFirestoreOptions['emulator'], private readonly watchSnapshot?: typeof import('firebase/firestore').onSnapshot) {
    this.uid = account.snapshot().user?.uid;
    this.authenticationEpoch = account.sessionEpoch?.() ?? 0;
    this.stopAccount = account.onChanged(() => {
      this.syncAccount();
    });
  }
  private syncAccount(): void {
    const uid = this.account.snapshot().user?.uid, authenticationEpoch = this.account.sessionEpoch?.() ?? 0;
    if (uid === this.uid && authenticationEpoch === this.authenticationEpoch) return;
    this.uid = uid; this.authenticationEpoch = authenticationEpoch; this.reset(); this.events.emit('change', { invalidated: true });
  }
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void {
    this.events.on('change', callback); return () => { this.events.off('change', callback); };
  }
  onMessagesChanged(callback: (signal?: MessageSignal) => void): () => void { this.events.on('messages', callback); return () => { this.events.off('messages', callback); }; }
  private reset(clearAuthentication = true): void {
    this.epoch++;
    for (const [leaseId, lease] of this.messageLeases) this.events.emit('snapshot', { leaseId, snapshot: {
      uid: lease.uid, teamId: lease.teamId, generation: lease.generation, kind: 'error', fromCache: true, error: 'INVALID_SESSION',
    } });
    this.host?.revoke();
    for (const stop of this.stops.values()) stop();
    this.stops.clear();
    this.failed.clear(); this.messageLeases.clear();
    const previousData = this.dataSession;
    this.dataSession = undefined; this.dataOpening = undefined;
    this.dataOperation = this.dataOperation.then(async () => { await previousData?.close(clearAuthentication); }).catch(() => undefined);
    const previous = this.session; this.session = undefined; this.opening = undefined;
    void previous?.close().catch(() => undefined);
  }
  private async connect(expectedUid: string): Promise<TeamsFirestoreSession> {
    this.syncAccount();
    const startedEpoch = this.epoch, restoring = !this.uid;
    await this.account.getIdToken(); // Also waits for the saved account to restore.
    this.syncAccount();
    if (!restoring && startedEpoch !== this.epoch) throw new Error('INVALID_SESSION');
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const sdk = this.sdk ??= await import('./firestore.js');
    if (this.uid !== expectedUid || startedEpoch !== this.epoch && !restoring) throw new Error('INVALID_SESSION');
    const { collection, onSnapshot, openTeamsFirestoreSession } = sdk;
    if (this.session) return this.session;
    if (this.opening) return this.opening;
    const uid = this.uid, epoch = this.epoch;
    const opening = (async () => {
      const credential = await this.request<{ uid: string; customToken: string }>('/session', {});
      if (epoch !== this.epoch || credential.uid !== uid) throw new Error('INVALID_SESSION');
      const session = this.session ?? await openTeamsFirestoreSession({ config: this.config, uid, customToken: credential.customToken, emulator: this.emulator });
      if (epoch !== this.epoch) { await session.close(); throw new Error('INVALID_SESSION'); }
      this.session = session;
      if (!this.stops.has('index')) this.stops.set('index', onSnapshot(collection(session.db, 'users', uid, 'teams'), { includeMetadataChanges: true }, snapshot => {
        if (epoch === this.epoch) this.events.emit('change', { invalidated: snapshot.metadata.fromCache });
      }, () => {
        if (epoch !== this.epoch) return;
        this.stops.get('index')?.(); this.stops.delete('index'); this.failed.set('index', 0);
        this.events.emit('change', { invalidated: true });
      }));
      return session;
    })();
    this.opening = opening;
    try { return await opening; } finally { if (this.opening === opening) this.opening = undefined; }
  }
  private async connectMessages(uid: string): Promise<import('./messageWindow.js').TeamsMessageSession> {
    this.syncAccount();
    if (this.uid !== uid) throw new Error('INVALID_SESSION');
    if (this.dataSession) return this.dataSession;
    if (this.dataOpening) return this.dataOpening;
    const epoch = this.epoch, previousOperation = this.dataOperation;
    let release!: () => void;
    this.dataOperation = new Promise<void>(resolve => { release = resolve; });
    const opening = (async () => {
      await previousOperation;
      if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
      let customToken: string | undefined;
      try {
        const credential = await this.request<{ uid: string; customToken: string }>('/session', {});
        if (credential.uid !== uid) throw new Error('INVALID_SESSION');
        customToken = credential.customToken;
      } catch (error) {
        if (error instanceof TeamsApiError && error.status < 500 || error instanceof Error && ['INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(error.message)) throw error;
        // A previously authenticated UID-private SDK session can still serve its cache.
      }
      this.syncAccount();
      if (epoch !== this.epoch || uid !== this.uid) throw new Error('INVALID_SESSION');
      const { openTeamsMessageSession } = await import('./messageWindow.js');
      const session = await openTeamsMessageSession({ config: this.config, uid, customToken, emulator: this.emulator }, () => {
        if (epoch !== this.epoch) return;
        this.dataSession = undefined;
        for (const [key, stop] of this.stops) if (key.startsWith('ui:')) { stop(); this.stops.delete(key); }
      });
      if (epoch !== this.epoch) { await session.close(true); throw new Error('INVALID_SESSION'); }
      this.dataSession = session; return session;
    })().finally(release);
    this.dataOpening = opening;
    try { return await opening; } finally { if (this.dataOpening === opening) this.dataOpening = undefined; }
  }
  /** `keepAfterReset` is only for a request whose answer is spent authority that cannot be asked for again (a start permit): the caller already holds the sending account's context and keeps the answer under that account. */
  private async request<T>(path = '', body?: unknown, keepAfterReset = false): Promise<T> {
    this.syncAccount();
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const uid = this.uid, epoch = this.epoch;
    const transport = teamsTransport(this.base, async () => {
      this.syncAccount();
      if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
      const token = await this.account.getIdToken();
      this.syncAccount();
      if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
      return token;
    });
    const result = await transport<T>(path, body);
    this.syncAccount();
    if (!keepAfterReset && (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid)) throw new Error('INVALID_SESSION');
    return result;
  }
  private async checked<T>(expectedUid: string, path = '', body?: unknown): Promise<T> {
    this.syncAccount();
    const currentUid = this.account.snapshot().user?.uid;
    if (currentUid !== expectedUid) throw new Error('INVALID_SESSION');
    const epoch = this.epoch;
    if (!this.uid) throw new Error('SIGN_IN_REQUIRED');
    const result = await this.request<T>(path, body);
    if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
    return result;
  }
  async datasets<T>(expectedUid: string, request: Record<string, unknown>): Promise<T> {
    if (request.action === 'teamVisualization') return this.checked<T>(expectedUid, '/visualizations', request.request);
    return request.action === 'page' ? this.checked<T>(expectedUid, '/dataset-pages', request.request) : this.checked<T>(expectedUid, '/datasets', request);
  }
  async data<T>(input: DataRequest, expectedUid: string): Promise<T> {
    return this.checked<T>(expectedUid, '/data', input);
  }
  async list(expectedUid: string): Promise<{ teams: TeamSummary[] }> { await this.connect(expectedUid); return this.checked(expectedUid); }
  async detail(teamId: string, expectedUid: string): Promise<TeamDetail> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(teamId)) throw new Error('INVALID_INPUT');
    const session = await this.connect(expectedUid), epoch = this.epoch;
    const current = await this.checked<TeamDetail>(expectedUid, `/${teamId}`);
    const { collection, onSnapshot } = this.sdk!;
    if (!this.stops.has(teamId) && this.failed.get(teamId) !== current.team.generation) {
      this.stops.set(teamId, onSnapshot(collection(session.db, 'teams', teamId, 'members'), { includeMetadataChanges: true }, snapshot => {
        if (epoch === this.epoch) this.events.emit('change', { invalidated: snapshot.metadata.fromCache });
      }, () => {
        if (epoch !== this.epoch) return;
        this.stops.get(teamId)?.(); this.stops.delete(teamId);
        this.failed.set(teamId, current.team.generation);
        this.events.emit('change', { invalidated: true });
      }));
    }
    return current;
  }
  /** The account's Firestore session with the SDK that opened it: Friends' owner-only listeners run in it, and nothing else here is exposed. */
  async firestore(expectedUid: string): Promise<{ db: Firestore; sdk: typeof import('./firestore.js') }> {
    const session = await this.connect(expectedUid);
    return { db: session.db, sdk: this.sdk! };
  }
  async messages<T>(command: MessageCommand, expectedUid: string): Promise<T> {
    if (this.account.snapshot().user?.uid !== expectedUid) throw new Error('INVALID_SESSION');
    if (!command || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.teamId) || !Number.isSafeInteger(command.generation) || command.generation < 1) throw new Error('INVALID_INPUT');
    if ('schemaVersion' in command) {
      if (command.schemaVersion !== 1 || !['listSharedBots', 'createConversation', 'submit', 'pauseSharedBot', 'answerWait', 'cancelTask', 'getTask', 'listTasks', 'listTriage', 'listHeldReplies', 'approveReply', 'rejectReply', 'readArtifact', 'listArtifactRevisions', 'reviseArtifact', 'listOperations', 'resolveOperation', 'readList', 'writeList', 'listHistory', 'decideWait', 'setConversationPolicy', 'setConversationAutoReply', 'setConversationBotScope', 'endTransfer',
        'createUpload', 'putUploadPart', 'finalizeUpload', 'cancelUpload', 'readAttachmentPart', 'revokeAttachment'].includes(command.action)) throw new Error('INVALID_INPUT');
      if (command.action === 'pauseSharedBot') this.host?.stopBinding(command.teamId, command.payload.sharedBotId);
    } else if (!['create', 'read', 'mutate'].includes(command.action)) throw new Error('INVALID_INPUT');
    await this.connect(expectedUid);
    const result = await this.checked<T>(expectedUid, '/messages', command);
    if (['create','mutate','createConversation','submit','pauseSharedBot','answerWait','cancelTask','approveReply','rejectReply','setConversationPolicy','setConversationAutoReply','setConversationBotScope','endTransfer','revokeAttachment'].includes(command.action)) this.events.emit('messages');
    return result;
  }
  async historyPage(uid:string,scope:MessageScope,conversationId:string,beforeSeq:number):Promise<TeamMessage[]> {
    validateHistoryPage(scope,conversationId,beforeSeq);
    const context=this.hostContext();
    if(!context || context.uid!==uid)throw new Error('INVALID_SESSION');
    const session=await this.connectMessages(uid);
    this.assertHostContext(context);
    const result=await session.historyPage(scope,conversationId,beforeSeq);
    this.assertHostContext(context);
    return result;
  }
  async watchMessages(leaseId: string, uid: string, scope: { teamId: string; generation: number }, conversationId: string): Promise<void> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(scope.teamId) || !Number.isSafeInteger(scope.generation) || scope.generation < 1
      || conversationId !== '' && !/^[A-Za-z0-9_-]{8,64}$/u.test(conversationId)) throw new Error('INVALID_INPUT');
    this.syncAccount();
    if (uid !== this.uid) throw new Error('INVALID_SESSION');
    const lease = { uid, ...scope, conversationId };
    this.messageLeases.set(leaseId, lease);
    try {
      await this.connectMessages(uid);
      if (this.messageLeases.get(leaseId) === lease) this.syncMessageWatches();
    } catch (error) { if (this.messageLeases.get(leaseId) === lease) this.messageLeases.delete(leaseId); throw error; }
  }
  releaseMessageWatches(prefix: string): void {
    for (const key of this.messageLeases.keys()) if (key.startsWith(prefix)) this.messageLeases.delete(key);
    this.syncMessageWatches();
  }
  private syncMessageWatches(): void {
    if (!this.dataSession) return;
    const wanted = new Map([...this.messageLeases].filter(([, lease]) => lease.uid === this.uid).map(([id, lease]) => [`ui:${id}`, lease]));
    for (const [key, stop] of this.stops) if (key.startsWith('ui:') && !wanted.has(key)) { stop(); this.stops.delete(key); }
    const epoch = this.epoch;
    for (const [key, lease] of wanted) if (!this.stops.has(key)) this.stops.set(key, this.dataSession.watch(key, lease, lease.conversationId, snapshot => {
      if (epoch !== this.epoch || !this.messageLeases.has(key.slice(3))) return;
      this.events.emit('snapshot', { leaseId: key.slice(3), snapshot });
      if (snapshot.kind === 'signal' || snapshot.kind === 'error') this.events.emit('messages', snapshot.kind === 'signal' ? snapshot.signal : undefined);
    }));
  }
  onMessageSnapshot(callback: (event: { leaseId: string; snapshot: MessageSnapshot }) => void): () => void {
    this.events.on('snapshot', callback); return () => { this.events.off('snapshot', callback); };
  }
  attachHost(host: LocalTeamHost): void { if (this.host) throw new Error('TEAM_HOST_ALREADY_ATTACHED'); this.host = host; }
  localBots(uid: string): Promise<LocalPublishableBot[]> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.localBots(uid); }
  selectSharedFiles(uid: string, instanceId: string): Promise<SelectedSharedFile[]> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.selectSharedFiles(uid, instanceId); }
  hostStatus(uid: string): Promise<SharedHostStatus> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.status(uid); }
  publishedKnowledge(uid: string, teamId: string, sharedBotId: string): Promise<PublishedKnowledge> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.publishedKnowledge(uid, teamId, sharedBotId); }
  triageTargets(uid: string, instanceId: string): Promise<TriageTarget[]> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.triageTargets(uid, instanceId); }
  triageLog(uid: string, request: TriageLogRequest): Promise<TriageLogPage> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.triageLog(uid, request); }
  datasetBotProgram(uid: string, scope: DatasetBotScope): Promise<DatasetTaskProgram> {
    this.assertHostContext({ uid, epoch: this.epoch });
    if (!this.host?.datasetBotProgram) throw new Error('TEAM_HOST_UNAVAILABLE');
    return this.host.datasetBotProgram(uid, scope);
  }
  publishLocalBot(uid: string, input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }> {
    this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.publishLocalBot(uid, input);
  }
  /** A handover changes what every Bot list shows, so the pages are told as after any other change. */
  async offerSharedBot(uid: string, input: OfferSharedBotInput): Promise<void> {
    this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE');
    try { await this.host.offerSharedBot(uid, input); } finally { this.events.emit('messages'); }
  }
  async acceptSharedBot(uid: string, input: AcceptSharedBotInput): Promise<{ sharedBotId: string; revision: number }> {
    this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE');
    try { return await this.host.acceptSharedBot(uid, input); } finally { this.events.emit('messages'); }
  }
  /** Main-only capabilities; no IPC branch exposes these methods or commands. */
  hostContext(): TeamAccountContext | undefined { this.syncAccount(); return this.uid && this.account.snapshot().user?.uid === this.uid ? { uid: this.uid, epoch: this.epoch } : undefined; }
  assertHostContext(context: TeamAccountContext): void {
    this.syncAccount();
    if (!this.uid || context.uid !== this.uid || context.epoch !== this.epoch || this.account.snapshot().user?.uid !== context.uid) throw new Error('INVALID_SESSION');
  }
  async hostPrivatePreviewPermit(context: TeamAccountContext, proof: PrivatePreviewProof): Promise<PrivatePreviewLease> {
    this.assertHostContext(context);
    const result = await this.checked<PrivatePreviewLease>(context.uid, '/data', { action: 'privatePreviewPermit', ...proof });
    this.assertHostContext(context);
    if (!validPreviewLease(result)) throw new Error('DATA_PREVIEW_UNAVAILABLE');
    return result;
  }

  async hostPrivatePreviewPage(context: TeamAccountContext, proof: PrivatePreviewProof, directory: string, signal: AbortSignal): Promise<PrivatePreviewPage> {
    const lease = await this.hostPrivatePreviewPermit(context, proof);
    const token = await this.account.getIdToken();
    this.assertHostContext(context);
    const page = await readPrivateDataPage(proof.source, lease, token, directory, signal);
    this.assertHostContext(context);
    await this.hostPrivatePreviewPermit(context, proof);
    return page;
  }

  async hostDataOffers(context: TeamAccountContext, proof: TeamDataProof): Promise<TeamDatasetOffer[]> {
    if (process.env.LINGXI_DATA_BOT_TOOLS !== '1') return [];
    this.assertHostContext(context);
    const result = await this.checked<{ datasets: TeamDatasetOffer[] }>(context.uid, '/data', { action: 'botDataOffers', ...proof });
    this.assertHostContext(context);
    const values = result.datasets;
    if (!Array.isArray(values) || values.length > 32
      || new Set(values.map(value => value?.dataset)).size !== values.length
      || values.some(value => !value || !/^[A-Za-z0-9_-]{1,64}$/u.test(value.dataset)
        || !Number.isSafeInteger(value.version) || value.version < 1 || typeof value.write !== 'boolean'
        || (value.handoffTargets !== undefined && (!Array.isArray(value.handoffTargets) || value.handoffTargets.length > 8
          || value.handoffTargets.some(id => typeof id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/u.test(id)))))) throw new Error('TEAM_HOST_PROTOCOL');
    return values;
  }

  async manageSources<T>(context: TeamAccountContext, command: Record<string, unknown>): Promise<T> {
    this.assertHostContext(context);
    const result = await this.checked<T>(context.uid, '/data', command);
    this.assertHostContext(context);
    return result;
  }

  async hostDataOperation(context: TeamAccountContext, proof: TeamDataProof, accessVersion: number,
    operation: Record<string, unknown>, stepId?: string): Promise<Record<string, unknown>> {
    this.assertHostContext(context);
    const result = await this.checked<Record<string, unknown>>(context.uid, '/data', operation.action === 'handoff'
      ? { action: 'botDataHandoff', ...proof, accessVersion, stepId, reference: operation.reference, sharedBotId: operation.sharedBotId }
      : { action: 'botDataOperation', ...proof, accessVersion, operation, ...(stepId ? { stepId } : {}) });
    this.assertHostContext(context);
    return result;
  }

  async hostMessages<T>(context: TeamAccountContext, command: Record<string, unknown>, openVisualization?: OpenBotVisualization): Promise<T> {
    this.assertHostContext(context);
    if (command.schemaVersion !== 1 || ![
      'listSharedBots', 'publishSharedBot', 'heartbeatDeployment', 'activateDeployment', 'listHostTasks',
      'prepareTask', 'validateTask', 'startTask', 'completeTask', 'holdReply', 'pauseTask', 'continueTask', 'getTask',
      'readTaskList', 'recordOperation', 'writeTaskList', 'readAttachmentPart',
      'putTransferPart', 'offerTransfer', 'acceptTransfer', 'readTransferPart', 'completeTransfer', 'declineTask', 'recordTriage', 'listTriage', 'bidReply', 'delegateTask', 'detachDelegation',
    ].includes(command.action as string)) throw new Error('INVALID_INPUT');
    const session = await this.connect(context.uid); this.assertHostContext(context);
    const { collection, onSnapshot } = this.sdk!;
    if (command.action === 'listHostTasks' && typeof command.teamId === 'string') {
      const key = `host:signals:${command.teamId}:${command.generation}`;
      if (!this.stops.has(key) && !this.failed.has(key)) {
        this.stops.set(key, (this.watchSnapshot ?? onSnapshot)(collection(session.db, 'teams', command.teamId, 'signals'), { includeMetadataChanges: true }, snapshot => {
          if (context.epoch === this.epoch && !snapshot.metadata.fromCache) this.events.emit('messages');
        }, () => {
          if (context.epoch !== this.epoch) return;
          this.stops.get(key)?.(); this.stops.delete(key); this.failed.set(key, command.generation as number);
          this.events.emit('change', { invalidated: true });
        }));
      }
    }
    // The server grants a model permit (first start or continue) once and answers a replay with `execute: false`: an account reset while the request is in flight must
    // still hand the permit to the connector, which saves it under the sending account and ends the task. Every other answer is discarded once the account moved on.
    // The context was asserted just above with no await since, so the request is sent under it.
    if (command.action === 'startTask' || command.action === 'continueTask') return this.request<T>('/messages', command, true);
    if (command.action === 'completeTask' && command.payload && typeof command.payload === 'object' && 'visualization' in command.payload) {
      const { visualization, ...payload } = command.payload;
      const artifact = typeof (payload as Record<string, unknown>).taskId === 'string'
        ? parseVisualizationArtifact(visualization, (payload as { taskId: string }).taskId) : undefined;
      const completion = { ...command, payload };
      const file = artifact && await openVisualization?.(artifact).catch(() => undefined);
      const call = <V>(path: string, body: Record<string, unknown>) => {
        this.assertHostContext(context);
        return this.checked<V>(context.uid, path, body);
      };
      if (!artifact || !file) return call<T>('/messages', completion);
      try {
        const { bindings } = artifact.parts[0].data;
        const parts = [{ mediaType: 'application/json', data: { version: 2, package: file.transfer.package, bindings } }];
        const body = { action: 'completeBot', completion, artifact: { artifactId: artifact.artifactId, name: artifact.name, parts } };
        const response = await call<VisualizationUpload | { status: 'completed'; result: T }>('/visualizations', { ...body, stage: 'prepare' });
        if (response.status === 'completed') return response.result;
        const { teamId, conversationId, generation } = response;
        try {
          await uploadVisualizationParts(<V>(part: Record<string, unknown>) =>
            call<V>('/visualizations', { ...part, teamId, conversationId, generation }), file.transfer, response);
        } catch (error) {
          // The commit path can settle this task as text if the channel limit was lowered.
          if (!(error instanceof TeamsApiError) || error.code !== 'VISUALIZATION_TOO_LARGE') throw error;
        }
        const finished = await call<{ status: 'completed'; result: T }>('/visualizations', { ...body, stage: 'commit' });
        if (finished.status !== 'completed') throw new Error('INVALID_DATA');
        return finished.result;
      } finally { await file.close(); }
    }
    const result = await this.checked<T>(context.uid, '/messages', command); this.assertHostContext(context); return result;
  }
  /** UI and every Bot read through the same UID/project message service and SDK cache. */
  async hostReadChat(context: TeamAccountContext, request: ChatReadRequest): Promise<ChatReadResult> {
    this.assertHostContext(context);
    // A hosted Bot service's sessions are read through the API: its records are not readable directly.
    if (isServiceId(request.teamId)) {
      if (this.serviceChat?.epoch !== context.epoch) this.serviceChat = { epoch: context.epoch, reader: createServiceChatReader(command => this.checked(context.uid, '/messages', command)) };
      const result = await this.serviceChat.reader.read(request); this.assertHostContext(context); return result;
    }
    const session = await this.connectMessages(context.uid); this.assertHostContext(context);
    const result = await session.read(request); this.assertHostContext(context); return result;
  }
  execute(command: TeamCommand, expectedUid: string): Promise<TeamResult> { return this.checked(expectedUid, '', command); }
  /** A caller's session or task command in a hosted Bot service's scope. Only what a caller may send there leaves this computer. */
  async serviceMessages<T>(command: unknown, expectedUid: string): Promise<T> {
    if (this.account.snapshot().user?.uid !== expectedUid) throw new Error('INVALID_SESSION');
    if (!isServiceMessageCommand(command)) throw new Error('INVALID_INPUT');
    return this.checked<T>(expectedUid, '/messages', command);
  }
  /** A command about a hosted Bot service itself (`/v1/services`). */
  async services<T>(command: ServiceCommand, expectedUid: string): Promise<T> {
    if (!['createService', 'updateService', 'setServiceOnline', 'withdrawService', 'getService', 'enterService', 'leaveService', 'reportService', 'listServices'].includes(command?.action)) throw new Error('INVALID_INPUT');
    this.syncAccount(); if (this.account.snapshot().user?.uid !== expectedUid || !this.uid) throw new Error('INVALID_SESSION');
    const uid = this.uid, epoch = this.epoch;
    const result = await servicesTransport(this.base, async () => {
      this.syncAccount(); if (epoch !== this.epoch) throw new Error('INVALID_SESSION');
      const token = await this.account.getIdToken();
      this.syncAccount(); if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
      return token;
    })<T>(command);
    this.syncAccount(); if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
    return result;
  }
  close(): void { this.stopAccount(); this.host?.close(); this.host = undefined; this.reset(false); this.events.removeAllListeners(); }
}
