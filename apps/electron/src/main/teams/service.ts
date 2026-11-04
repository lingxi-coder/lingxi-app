import { EventEmitter } from 'node:events';
import type { FirebaseOptions } from 'firebase/app';
import { TeamsApiError, teamsTransport, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../../packages/teams';
import type { MessageSignal, MessageSnapshot, MessageCommand } from '../../../../../packages/teams/messages';
import type { BindLocalTeamBotInput, LocalTeamBot } from '../../../../../packages/teams/bots';
import type { TeamsFirestoreSession, TeamsFirestoreOptions } from './firestore.js';

interface Account {
  snapshot(): { user: { uid: string } | null };
  sessionEpoch?(): number;
  getIdToken(): Promise<string>;
  onChanged(callback: () => void): () => void;
}
export interface TeamAccountContext { uid: string; epoch: number }
export interface LocalTeamHost {
  localBots(uid: string): Promise<LocalTeamBot[]>;
  bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }>;
  invalidate(): void;
  stopBinding(teamId: string, botKey: string): void;
  runStatus?(uid: string, teamId: string, conversationId: string, triggerId: string): string | undefined;
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
    this.host?.invalidate();
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
  private async request<T>(path = '', body?: unknown): Promise<T> {
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
    if (epoch !== this.epoch || this.account.snapshot().user?.uid !== uid) throw new Error('INVALID_SESSION');
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
  async messages<T>(command: MessageCommand, expectedUid: string): Promise<T> {
    if (this.account.snapshot().user?.uid !== expectedUid) throw new Error('INVALID_SESSION');
    if (!command || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.teamId) || !Number.isSafeInteger(command.generation) || command.generation < 1) throw new Error('INVALID_INPUT');
    // Runtime checks are required: IPC values do not acquire TypeScript's union.
    if (!['create', 'read', 'mutate', 'bots', 'unbindBot', 'botState', 'botStates', 'botAnswer', 'recoverBot'].includes(command.action)) throw new Error('INVALID_INPUT');
    if (command.action === 'unbindBot') this.host?.stopBinding(command.teamId, command.botKey);
    await this.connect(expectedUid);
    const result = await this.checked<T>(expectedUid, '/messages', command);
    if (command.action === 'botState' && result && typeof result === 'object'
      && (result as Record<string, unknown>).ownerUid === expectedUid) {
      const hostStatus = this.host?.runStatus?.(expectedUid, command.teamId, command.conversationId, command.triggerId);
      if (hostStatus) return { ...result, hostStatus };
    }
    if (command.action === 'unbindBot' || command.action === 'recoverBot') this.host?.invalidate();
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
  notifyHostChanged(): void { this.events.emit('messages'); }
  localBots(uid: string): Promise<LocalTeamBot[]> { this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.localBots(uid); }
  bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }> {
    this.assertHostContext({ uid, epoch: this.epoch }); if (!this.host) throw new Error('TEAM_HOST_UNAVAILABLE'); return this.host.bindLocalBot(uid, input);
  }
  /** Main-only capabilities; no IPC branch exposes these methods or commands. */
  hostContext(): TeamAccountContext | undefined { this.syncAccount(); return this.uid && this.account.snapshot().user?.uid === this.uid ? { uid: this.uid, epoch: this.epoch } : undefined; }
  assertHostContext(context: TeamAccountContext): void {
    this.syncAccount();
    if (!this.uid || context.uid !== this.uid || context.epoch !== this.epoch || this.account.snapshot().user?.uid !== context.uid) throw new Error('INVALID_SESSION');
  }
  async hostToken(context: TeamAccountContext): Promise<string> {
    this.assertHostContext(context);
    const token = await this.account.getIdToken();
    this.assertHostContext(context);
    if (!token || token.length > 32 * 1024) throw new Error('INVALID_SESSION');
    return token;
  }
  async hostMessages<T>(context: TeamAccountContext, command: Record<string, unknown>): Promise<T> {
    this.assertHostContext(context);
    if (!['bots', 'bindBot', 'unbindBot', 'heartbeatBot', 'botQueue', 'botConversations'].includes(command.action as string)) throw new Error('INVALID_INPUT');
    const session = await this.connect(context.uid);
    this.assertHostContext(context);
    const { collection, query, where, onSnapshot } = this.sdk!;
    // SDK signals wake discovery; transactional HTTP queues arbitrate execution.
    // Bot cards include owner-only disabled revisions and are intentionally HTTP
    // only. Rules deny raw Bot SDK reads; do not attach an always-denied watch.
    if (command.action === 'botConversations' && typeof command.teamId === 'string') {
      const key = `host:${command.action}:${command.teamId}:${command.generation}`;
      if (!this.stops.has(key) && !this.failed.has(key)) {
        const emit = () => { if (context.epoch === this.epoch) this.events.emit('messages'); };
        const fail = () => { if (context.epoch === this.epoch && !this.failed.has(key)) {
          for (const source of [key, `${key}:signals`]) { this.stops.get(source)?.(); this.stops.delete(source); }
          this.failed.set(key, command.generation as number);
          this.events.emit('change', { invalidated: true });
        } };
        const source = query(collection(session.db, 'teams', command.teamId, 'conversations'), where('memberUids', 'array-contains', context.uid));
        this.stops.set(key, (this.watchSnapshot ?? onSnapshot)(source, { includeMetadataChanges: true }, snapshot => { if (!snapshot.metadata.fromCache) emit(); }, fail));
        this.stops.set(`${key}:signals`, (this.watchSnapshot ?? onSnapshot)(collection(session.db, 'teams', command.teamId, 'signals'), { includeMetadataChanges: true }, snapshot => { if (!snapshot.metadata.fromCache) emit(); }, fail));
      }
    }
    const result = await this.checked<T>(context.uid, '/messages', command);
    this.assertHostContext(context); return result;
  }
  execute(command: TeamCommand, expectedUid: string): Promise<TeamResult> { return this.checked(expectedUid, '', command); }
  close(): void { this.stopAccount(); this.host?.close(); this.host = undefined; this.reset(false); this.events.removeAllListeners(); }
}
